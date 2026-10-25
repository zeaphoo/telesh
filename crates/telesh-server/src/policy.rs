//! 版本策略：一个账户能开多少隧道、能用什么子域、每月多少流量。
//!
//! 开源版默认实现是 [`Unlimited`]——行为与没有这个接缝时完全一样。
//! 商业版（`telesh_super`）在 `main` 里先 [`crate::state::set_policy`] 换掉它，
//! 再开始服务；引擎其余部分只认这里的 trait，不认版本。
//!
//! 判定点只有三个（见调用点注释）：
//!   - `control::register_http`：HTTP 隧道数上限 + 子域规则
//!   - `control::register_tcp`：TCP 隧道数上限
//!   - `ingress::open_stream`：流量额度（开流放行 + 结束后记账）
//!
//! 设计约定：`on_traffic` 必须是廉价同步调用（只累加），判定放在
//! `allow_new_stream`，这样数据面热路径上不做任何 IO。

use telesh_protocol::{random_subdomain, validate_subdomain};

/// 子域规则。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SubdomainRule {
    /// 请求什么给什么（先到先得）；没请求就随机。
    #[default]
    Any,
    /// 必须以该前缀开头，例如免费用户只能拿 `u-xxxx`。
    MustStartWith(String),
    /// 固定这一个名字，忽略请求（例如单租户私有化部署）。
    Fixed(String),
}

/// 一个账户当前生效的上限。`None` 一律表示「不限」。
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_http_tunnels: Option<u32>,
    pub max_tcp_tunnels: Option<u32>,
    pub monthly_traffic_bytes: Option<u64>,
    pub subdomain: SubdomainRule,
    pub custom_domain: bool,
}

impl Limits {
    pub fn unlimited() -> Self {
        Self {
            max_http_tunnels: None,
            max_tcp_tunnels: None,
            monthly_traffic_bytes: None,
            subdomain: SubdomainRule::Any,
            custom_domain: true,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::unlimited()
    }
}

/// 子域被拒的原因。调用方统一回 `tunnel_resp.invalid`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubdomainError {
    /// 名字本身不合法（字符、长度、开头/结尾的 `-`）。
    Invalid,
    /// 名字合法，但不满足该账户的规则（例如免费用户没带前缀）。
    Denied,
}

/// 版本策略。只有 [`Policy::limits`] 是必须实现的。
pub trait Policy: Send + Sync {
    /// 该账户当前的上限。被高频调用（每次发布、每次开流），实现方需自备缓存。
    fn limits(&self, account: &str) -> Limits;

    /// 流量记账：服务端看得到的（HTTP/TCP）与 agent 上报的（P2P）都走这里。
    /// 必须廉价、同步，不做 IO。
    fn on_traffic(&self, account: &str, up: u64, down: u64) {
        let _ = (account, up, down);
    }

    /// 是否放行一条新的公网连接。额度用尽时返回 `false`。
    fn allow_new_stream(&self, account: &str) -> bool {
        let _ = account;
        true
    }

    /// 把 agent 请求的子域变成最终子域。
    ///
    /// 默认按 [`Limits::subdomain`] 推导，一般不覆盖；要改规则就改 `limits()`。
    fn resolve_subdomain(
        &self,
        account: &str,
        requested: Option<&str>,
    ) -> Result<String, SubdomainError> {
        apply_subdomain_rule(&self.limits(account).subdomain, account, requested)
    }
}

fn apply_subdomain_rule(
    rule: &SubdomainRule,
    account: &str,
    requested: Option<&str>,
) -> Result<String, SubdomainError> {
    match rule {
        SubdomainRule::Any => match requested {
            Some(s) if validate_subdomain(s) => Ok(s.to_string()),
            Some(_) => Err(SubdomainError::Invalid),
            None => Ok(random_subdomain()),
        },
        SubdomainRule::MustStartWith(prefix) => {
            // 已经带了前缀就不重复加；没带就补上，最终名字仍然要合法。
            let name = match requested {
                Some(s) => {
                    let tail = s.strip_prefix(prefix.as_str()).unwrap_or(s);
                    format!("{prefix}{tail}")
                }
                None => format!("{prefix}{account}"),
            };
            if !validate_subdomain(&name) {
                return Err(SubdomainError::Invalid);
            }
            Ok(name)
        }
        SubdomainRule::Fixed(name) => {
            if validate_subdomain(name) {
                Ok(name.clone())
            } else {
                Err(SubdomainError::Denied)
            }
        }
    }
}

/// 开源版：不限额度，子域先到先得。
pub struct Unlimited;

impl Policy for Unlimited {
    fn limits(&self, _account: &str) -> Limits {
        Limits::unlimited()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(rule: SubdomainRule, account: &str, requested: Option<&str>) -> Result<String, SubdomainError> {
        apply_subdomain_rule(&rule, account, requested)
    }

    #[test]
    fn any_keeps_the_request() {
        assert_eq!(resolve(SubdomainRule::Any, "000012345", Some("web")), Ok("web".into()));
        assert_eq!(resolve(SubdomainRule::Any, "000012345", Some("BAD")), Err(SubdomainError::Invalid));
        assert_eq!(resolve(SubdomainRule::Any, "000012345", Some("-x")), Err(SubdomainError::Invalid));
        // 没请求就随机，长度固定且合法。
        let r = resolve(SubdomainRule::Any, "000012345", None).expect("random");
        assert!(validate_subdomain(&r));
    }

    #[test]
    fn must_start_with_prefixes_once() {
        let rule = SubdomainRule::MustStartWith("u-".into());
        assert_eq!(resolve(rule.clone(), "000012345", Some("web")), Ok("u-web".into()));
        // 已经带了前缀不会变成 u-u-web。
        assert_eq!(resolve(rule.clone(), "000012345", Some("u-web")), Ok("u-web".into()));
        // 没请求时用账户句柄。
        assert_eq!(resolve(rule, "000012345", None), Ok("u-000012345".into()));
    }

    #[test]
    fn fixed_ignores_the_request() {
        let rule = SubdomainRule::Fixed("tunnel".into());
        assert_eq!(resolve(rule, "000012345", Some("web")), Ok("tunnel".into()));
    }
}
