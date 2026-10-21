//! 本机 agent 的 HTTP 客户端。
//!
//! 只和 `127.0.0.1:<ui-port>` 说话,不需要 TLS。用最小的 HTTP/1.0 客户端,
//! 避免为了一个本机请求把 reqwest/rustls 带进 agent 的构建。
//!
//! 连接失败归类成 [`AgentError`],由 `main` 打印成可操作的提示,而不是 anyhow
//! 的 cause chain —— 参考 podman/docker 的「Cannot connect to ...」。

use std::fmt;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::anyhow;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// agent 不可用 / 请求失败。`Display` 直接面向用户。
#[derive(Debug)]
pub(crate) enum AgentError {
    /// 端口没人监听(连接被拒绝)。
    NotRunning {
        addr: SocketAddr,
        source: std::io::Error,
    },
    /// 端口上有 HTTP 服务,但不是 telesh。
    Foreign {
        addr: SocketAddr,
        server: Option<String>,
    },
    /// 是 telesh,但没有 `/api/instance`(旧版本)。
    TooOld { addr: SocketAddr },
    /// 连上了,但请求返回非 2xx。
    Http {
        method: &'static str,
        path: String,
        status: u16,
        body: String,
    },
    /// 超时、连接中断、JSON 解析失败等。
    Other(anyhow::Error),
}

impl AgentError {
    /// agent 本身不可用(而不是某次请求失败)。给 `main` 选退出码用。
    pub(crate) fn is_unavailable(&self) -> bool {
        matches!(
            self,
            AgentError::NotRunning { .. } | AgentError::Foreign { .. } | AgentError::TooOld { .. }
        )
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::NotRunning { addr, .. } => {
                write!(f, "telesh agent 没有在运行(http://{addr})。\n\n启动它:\n")?;
                #[cfg(target_os = "linux")]
                write!(
                    f,
                    "  systemctl --user start telesh    # 用户服务\n  \
                     sudo systemctl start telesh      # 系统服务\n"
                )?;
                writeln!(f, "  telesh                           # 前台运行")?;
                #[cfg(not(target_os = "linux"))]
                writeln!(f, "\n或用系统服务 / 桌面 app 启动。")?;
                write!(f, "\n面板在别的端口?加 `--ui-port <PORT>`。")
            }
            AgentError::Foreign { addr, server } => {
                write!(f, "http://{addr} 上不是 telesh")?;
                if let Some(server) = server {
                    write!(f, "(Server: {server})")?;
                }
                write!(f, "。\n\n")?;
                if server.is_some() {
                    write!(
                        f,
                        "端口被别的服务占用了。换一个面板端口后重试:\n  \
                         telesh --ui-port <PORT> <COMMAND>"
                    )
                } else {
                    write!(
                        f,
                        "- 面板在别的端口?加 `--ui-port <PORT>` 后重试。\n  \
                         - 如果这是旧版 telesh(没有 /api/instance),升级后再试。"
                    )
                }
            }
            AgentError::TooOld { addr } => write!(
                f,
                "http://{addr} 上的 telesh 版本过旧,没有 `/api/instance`。\n\n\
                 升级 telesh 后再试。"
            ),
            AgentError::Http {
                method,
                path,
                status,
                body,
            } => write!(
                f,
                "{method} {path}: agent 返回 HTTP {status}: {}",
                body.trim()
            ),
            AgentError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AgentError::NotRunning { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// 一个最小的、只面向本机 agent 的 HTTP 客户端。
pub(crate) struct AgentClient {
    addr: SocketAddr,
}

impl AgentClient {
    /// 连接并确认端口后面真的是 telesh(与 `instance.rs`、桌面外壳同一指纹)。
    pub(crate) async fn connect(ui_port: u16) -> Result<Self, AgentError> {
        Self::probe(ui_port).await?;
        Ok(Self {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, ui_port)),
        })
    }

    /// 探测 `/api/instance`,返回实例信息。`telesh open` 直接用它。
    pub(crate) async fn probe(ui_port: u16) -> Result<crate::instance::Instance, AgentError> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, ui_port));
        let client = Self { addr };
        let res = client.request("GET", "/api/instance", None).await?;

        if !(200..300).contains(&res.status) {
            // 旧版 telesh 没有 /api/instance;如果它还带着 telesh 的 Server 头,
            // 就能明确说是版本旧,而不是把端口误判成陌生服务。
            return if is_telesh_server(&res.server) {
                Err(AgentError::TooOld { addr })
            } else {
                Err(AgentError::Foreign {
                    addr,
                    server: res.server,
                })
            };
        }
        match serde_json::from_slice::<crate::instance::Instance>(&res.body) {
            Ok(info) if info.app == "telesh" => Ok(info),
            _ if is_telesh_server(&res.server) => Err(AgentError::TooOld { addr }),
            _ => Err(AgentError::Foreign {
                addr,
                server: res.server,
            }),
        }
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, AgentError> {
        let res = self.request("GET", path, None).await?;
        decode("GET", path, res)
    }

    pub(crate) async fn put<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        payload: &B,
    ) -> Result<T, AgentError> {
        let body = serde_json::to_vec(payload)
            .map_err(|e| AgentError::Other(anyhow!("{path}: 序列化请求失败: {e}")))?;
        let res = self.request("PUT", path, Some(body)).await?;
        decode("PUT", path, res)
    }

    async fn request(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<HttpResponse, AgentError> {
        // 连接阶段单独判错:拒绝连接 = agent 没在跑;连上后出错是另一类问题。
        let mut stream =
            match tokio::time::timeout(REQUEST_TIMEOUT, tokio::net::TcpStream::connect(self.addr))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) if not_running(&e) => {
                    return Err(AgentError::NotRunning {
                        addr: self.addr,
                        source: e,
                    })
                }
                Ok(Err(e)) => {
                    return Err(AgentError::Other(anyhow!(
                        "连接 http://{} 失败: {e}",
                        self.addr
                    )))
                }
                Err(_) => {
                    return Err(AgentError::Other(anyhow!(
                        "连接 http://{} 超时;agent 可能卡住了",
                        self.addr
                    )))
                }
            };

        let exchange = async {
            // HTTP/1.0 + `Connection: close`:响应不会被 chunked,读到底就是 body。
            let mut head =
                format!("{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
            if let Some(body) = &body {
                head.push_str("Content-Type: application/json\r\n");
                head.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            head.push_str("\r\n");
            stream.write_all(head.as_bytes()).await?;
            if let Some(body) = body {
                stream.write_all(&body).await?;
            }

            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await?;
            Ok::<_, std::io::Error>(raw)
        };

        let raw = match tokio::time::timeout(REQUEST_TIMEOUT, exchange).await {
            Ok(Ok(raw)) => raw,
            Ok(Err(e)) => {
                return Err(AgentError::Other(anyhow!(
                    "请求 http://{} 失败: {e}",
                    self.addr
                )))
            }
            Err(_) => return Err(AgentError::Other(anyhow!("请求 http://{} 超时", self.addr))),
        };
        parse_http_response(&raw).map_err(AgentError::Other)
    }
}

fn decode<T: DeserializeOwned>(
    method: &'static str,
    path: &str,
    res: HttpResponse,
) -> Result<T, AgentError> {
    if !(200..300).contains(&res.status) {
        return Err(AgentError::Http {
            method,
            path: path.to_string(),
            status: res.status,
            body: String::from_utf8_lossy(&res.body).into_owned(),
        });
    }
    serde_json::from_slice(&res.body)
        .map_err(|e| AgentError::Other(anyhow!("{path}: 返回的不是合法 JSON: {e}")))
}

fn is_telesh_server(server: &Option<String>) -> bool {
    server
        .as_deref()
        .is_some_and(|s| s.to_ascii_lowercase().starts_with("telesh/"))
}

fn not_running(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::ConnectionRefused
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::NotFound
    )
}

/// 只处理 `Content-Length` / 连接关闭这类简单响应 —— agent 的 JSON API 都是这种。
struct HttpResponse {
    status: u16,
    server: Option<String>,
    body: Vec<u8>,
}

fn parse_http_response(raw: &[u8]) -> Result<HttpResponse, anyhow::Error> {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("agent 返回了非 HTTP 响应"))?;
    let head =
        std::str::from_utf8(&raw[..sep]).map_err(|_| anyhow!("agent 返回了非 UTF-8 响应头"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("agent 返回了无法解析的状态行"))?;
    let server = head
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("server:"))
        .map(|line| line[7..].trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(HttpResponse {
        status,
        server,
        body: raw[sep + 4..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_server_and_body() {
        let raw = b"HTTP/1.1 200 OK\r\nServer: nginx/1.27\r\nContent-Length: 2\r\n\r\n{}";
        let res = parse_http_response(raw).unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(res.server.as_deref(), Some("nginx/1.27"));
        assert_eq!(res.body, b"{}");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_http_response(b"not http").is_err());
    }

    #[test]
    fn telesh_server_header_is_recognized() {
        assert!(is_telesh_server(&Some("telesh/0.0.9".into())));
        assert!(!is_telesh_server(&Some("nginx/1.27".into())));
        assert!(!is_telesh_server(&None));
    }

    #[test]
    fn connection_refused_is_not_running() {
        let e = std::io::Error::from(ErrorKind::ConnectionRefused);
        assert!(not_running(&e));
        let e = std::io::Error::from(ErrorKind::TimedOut);
        assert!(!not_running(&e));
    }
}
