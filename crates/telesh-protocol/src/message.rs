//! 控制面消息(客户端 ↔ 服务端)。
//!
//! 协议定义见 `docs/design-control.md`:控制消息编码为 JSON,信封是 serde 的
//! adjacent tagging —— `{"type":"hello","data":{...}}`,见 [`Frame`]。
//! 数据面的信令流消息见 [`crate::signal`]。

use iroh_base::EndpointAddr;
use serde::{Deserialize, Serialize};

/// 单条消息的字节上限(sanity check;WebSocket 自己负责分帧)。
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// 未指定子域时随机生成的子域长度。
pub const SUBDOMAIN_LEN: usize = 8;

// ---------------------------------------------------------------------------
// 字节字段的编码
//
// 规则:固定 32 字节的密钥 / 标识用 **hex**(和 `EndpointId` 的显示一致);
// 不透明变长字节(session / token / 签名)用 **base64**。
// ---------------------------------------------------------------------------

/// `Vec<u8>` ↔ base64 字符串。
pub mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    fn engine() -> base64::engine::general_purpose::GeneralPurpose {
        base64::engine::general_purpose::STANDARD
    }

    pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&engine().encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        engine().decode(s.as_bytes()).map_err(serde::de::Error::custom)
    }
}

/// `Option<Vec<u8>>` ↔ base64 字符串或 null。
pub mod b64_opt {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// `Option<[u8; 32]>` ↔ hex 字符串或 null。
pub mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    fn to_hex(b: &[u8]) -> String {
        let mut s = String::with_capacity(b.len() * 2);
        for x in b {
            s.push_str(&format!("{x:02x}"));
        }
        s
    }

    fn from_hex(s: &str) -> Result<[u8; 32], String> {
        if s.len() != 64 {
            return Err(format!("expected 64 hex chars, got {}", s.len()));
        }
        let mut out = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let part = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
            out[i] = u8::from_str_radix(part, 16).map_err(|e| e.to_string())?;
        }
        Ok(out)
    }

    pub fn serialize<S: Serializer>(v: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&to_hex(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => from_hex(&s).map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// `EndpointAddr` ↔ `{"id": "<hex>", "addrs": ["relay:https://…", "ip:1.2.3.4:80"]}`。
///
/// 显式映射,不依赖 iroh-base 自己的 serde 表示(那个会随 iroh 版本变)。
pub mod endpoint_addr {
    use std::{collections::BTreeSet, net::SocketAddr, str::FromStr};

    use iroh_base::{CustomAddr, EndpointAddr, EndpointId, RelayUrl, TransportAddr};
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

    #[derive(Serialize, Deserialize)]
    struct Repr {
        id: String,
        addrs: Vec<String>,
    }

    pub fn serialize<S: Serializer>(v: &EndpointAddr, s: S) -> Result<S::Ok, S::Error> {
        Repr {
            id: v.id.to_string(),
            addrs: v.addrs.iter().map(|a| a.to_string()).collect(),
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<EndpointAddr, D::Error> {
        let repr = Repr::deserialize(d)?;
        let id = EndpointId::from_str(&repr.id).map_err(D::Error::custom)?;
        let mut addrs = BTreeSet::new();
        for raw in repr.addrs {
            addrs.insert(parse_addr(&raw).map_err(D::Error::custom)?);
        }
        Ok(EndpointAddr { id, addrs })
    }

    fn parse_addr(s: &str) -> Result<TransportAddr, String> {
        let (kind, rest) = s
            .split_once(':')
            .ok_or_else(|| format!("address is missing a kind prefix: {s:?}"))?;
        match kind {
            "relay" => RelayUrl::from_str(rest)
                .map(TransportAddr::Relay)
                .map_err(|e| e.to_string()),
            "ip" => SocketAddr::from_str(rest)
                .map(TransportAddr::Ip)
                .map_err(|e| e.to_string()),
            "custom" => CustomAddr::from_str(rest)
                .map(TransportAddr::Custom)
                .map_err(|e| e.to_string()),
            other => Err(format!("unknown address kind {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------
// 消息结构
// ---------------------------------------------------------------------------

/// Agent → server:取消一条已发布的隧道。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelClose {
    pub kind: TunnelKind,
    /// `http`:子域;`tcp`:公网端口(十进制字符串);`p2p`:服务名。
    pub key: String,
}

/// 客户端类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Agent,
    Lookup,
}

/// 认证方式。目前只实现了密钥对。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    Keypair,
}

/// 登录签名的域分隔符。
pub const LOGIN_DOMAIN: &[u8] = b"telesh-login-v1";

/// 客户端要签的消息:域分隔 + 绑定 `server_id`,防止在另一台服务器上重放。
pub fn login_message(server_id: &[u8; 32], nonce: &[u8; 32], ts: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(LOGIN_DOMAIN.len() + 32 + 32 + 8);
    m.extend_from_slice(LOGIN_DOMAIN);
    m.extend_from_slice(server_id);
    m.extend_from_slice(nonce);
    m.extend_from_slice(&ts.to_be_bytes());
    m
}

/// Server → client:签这个来登录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub nonce: [u8; 32],
    /// 稳定的服务器标识,混进被签名的消息里。
    pub server_id: [u8; 32],
    /// Unix 秒,限制挑战的有效期。
    pub ts: u64,
}

/// Client → server:持有私钥的证明(Ed25519,64 字节,base64)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    #[serde(with = "b64")]
    pub signature: Vec<u8>,
}

/// Server → client:认证通过,这是你的会话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthOk {
    /// 9 位账户句柄,左侧补零(如 `"000012345"`)。
    pub account: String,
    /// 不透明会话令牌。
    #[serde(with = "b64")]
    pub session: Vec<u8>,
    /// 会话过期时间(unix 秒)。
    pub expires_at: u64,
    /// 账户的友好节点名。
    pub display_name: String,
}

/// Agent → server:校验一个不透明的访问令牌。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCheck {
    /// 用于把回复和请求对上。
    pub id: u64,
    pub account: String,
    pub name: String,
    #[serde(with = "b64")]
    pub token: Vec<u8>,
}

/// Server → agent:[`TokenCheck`] 的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCheckResp {
    pub id: u64,
    pub ok: bool,
    /// 令牌过期时间(unix 秒);agent 会把授权缓存到那时。
    pub expires_at: u64,
}

/// Server → client:认证失败。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthError {
    BadSignature,
    BadSession,
    SessionExpired,
    EnrollmentRequired,
    EnrollmentRejected,
    UnsupportedMethod,
    Error(String),
}

/// Server → agent:某个服务名的归属结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum P2PAnnounceResp {
    /// 这个名字现在归该 endpoint 所有。
    Ok { name: String },
    /// 已被另一个 endpoint 占用。
    NameTaken { name: String },
    /// 名字不合法。
    InvalidName { name: String },
}

/// Agent → server:服务端看不到的流量计数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficReport {
    /// 计数所属的服务名。
    pub name: String,
    /// 发给 peer 的字节数。
    pub up: u64,
    /// 从 peer 收到的字节数。
    pub down: u64,
}

/// Agent → server:路由器端口映射(UPnP/PCP/NAT-PMP)打开的公网 TCP 端点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpMapped {
    pub name: String,
    /// 网关报告的公网 `IP:port`。
    pub addr: std::net::SocketAddr,
    /// 被映射的本地端口。
    pub local_port: u16,
}

/// Peer → server:打开一条 lookup-only 控制连接(不注册任何东西)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupHello {
    /// 服务端配置了主密钥 / lookup 密钥时必须匹配。
    pub auth_key: Option<String>,
    /// 临时标识,只用于日志。
    pub agent_id: String,
}

/// Agent → server:注册一个 peer 可以直连的 iroh 服务。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PAnnounce {
    /// peer 用来查目录的服务名。
    pub name: String,
    /// iroh 寻址信息。
    #[serde(with = "endpoint_addr")]
    pub addr: EndpointAddr,
    /// 被暴露的本地端口,只用于 dashboard。
    pub local_port: u16,
}

/// Peer → server:在某个账户下查一个服务。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PLookup {
    /// 9 位账户句柄,左侧补零。
    pub account: String,
    /// 账户内的服务名。
    pub name: String,
}

/// [`P2PLookup`] 的结果,也是订阅推送的载荷。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum P2PAddrResp {
    Found {
        account: String,
        name: String,
        /// 服务所属节点的友好名,用于显示/日志。
        owner_name: String,
        #[serde(with = "endpoint_addr")]
        addr: EndpointAddr,
        /// 针对 `account/name` 的短期不透明访问令牌。
        #[serde(with = "b64_opt")]
        token: Option<Vec<u8>>,
    },
    NotFound,
}

/// 首条客户端消息:协商身份与认证方式。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub client_kind: ClientKind,
    pub method: AuthMethod,
    /// 密钥对登录用的 `EndpointId`(Ed25519 公钥,hex)。
    #[serde(with = "hex32")]
    pub public_key: Option<[u8; 32]>,
    /// 快路径用的不透明会话令牌。
    #[serde(with = "b64_opt")]
    pub session: Option<Vec<u8>>,
    /// 服务端配置了注册密钥时,首次注册必须带上。
    pub enrollment: Option<String>,
    /// 友好节点名。
    pub display_name: Option<String>,
}

/// 握手结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerHello {
    Success {
        /// 9 位账户句柄(连接级身份;隧道级信息在各自的注册回复里)。
        agent_id: String,
        /// 客户端该用的 iroh relay(服务端下发);`None` 表示用 iroh 默认(n0)。
        relay_url: Option<String>,
        /// 数据面地址 `<host>:<port>`;`None` 表示服务端没有数据面。
        data_addr: Option<String>,
    },
    AuthFailed,
    Error(String),
}

impl ServerHello {
    /// 服务端下发的 relay URL。
    pub fn relay_url(&self) -> Option<&str> {
        match self {
            ServerHello::Success { relay_url, .. } => relay_url.as_deref(),
            _ => None,
        }
    }

    /// 服务端下发的数据面地址。
    pub fn data_addr(&self) -> Option<&str> {
        match self {
            ServerHello::Success { data_addr, .. } => data_addr.as_deref(),
            _ => None,
        }
    }
}

/// 隧道类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    Http,
    Tcp,
    P2p,
}

/// Agent → server:发布一条隧道。
///
/// `id` 由 agent 生成,响应原样带回 —— 多隧道可以并发注册,不用靠顺序配对。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelPublish {
    pub id: u64,
    pub kind: TunnelKind,
    /// agent 侧的目标标识(本地端口),服务端只存不解释,开流时原样回传。
    pub target: String,
    /// `http`:期望子域;`p2p`:服务名;`tcp`:未用。
    pub name: Option<String>,
    /// `tcp`:期望公网端口;其余未用。
    pub remote_port: Option<u16>,
}

/// [`TunnelPublish`] 的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TunnelResult {
    Http { sub_domain: String, hostname: String },
    Tcp { port: u16 },
    /// 子域 / 服务名 / 端口被占用。
    NameTaken,
    /// 名字或端口不合法。
    Invalid,
    Error(String),
}

/// Server → agent:某条发布请求的结果(按 `id` 配对)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelResp {
    pub id: u64,
    #[serde(flatten)]
    pub result: TunnelResult,
}

/// 线上控制消息。信封是 serde 的 adjacent tagging:
/// `{"type": "hello", "data": {...}}`。见 `docs/design-control.md` §2。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Frame {
    Hello(ClientHello),
    ServerHello(ServerHello),
    Ping,
    Pong,
    P2PAnnounce(P2PAnnounce),
    P2PAnnounceResp(P2PAnnounceResp),
    P2PLookup(P2PLookup),
    P2PAddr(P2PAddrResp),
    Lookup(LookupHello),
    IrohReady(#[serde(with = "endpoint_addr")] EndpointAddr),
    Traffic(TrafficReport),
    TcpMapped(TcpMapped),
    AuthChallenge(AuthChallenge),
    AuthResponse(AuthResponse),
    AuthOk(AuthOk),
    AuthError(AuthError),
    TokenCheck(TokenCheck),
    TokenCheckResp(TokenCheckResp),
    /// 多隧道:发布 / 结果 / 取消。
    TunnelPublish(TunnelPublish),
    TunnelResp(TunnelResp),
    TunnelClose(TunnelClose),
}

/// 随机子域(`[a-z0-9]`)。
pub fn random_subdomain() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..SUBDOMAIN_LEN)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// 校验子域 / 服务名:`[a-z0-9-]`,1..=63 字符,不以 `-` 开头或结尾。
pub fn validate_subdomain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}
