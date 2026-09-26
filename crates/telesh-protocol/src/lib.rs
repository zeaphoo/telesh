//! telesh-protocol — telesh agent 与 server 之间的线上协议。
//!
//! * **控制面**(`docs/design-control.md`):JSON 文本,信封
//!   `{"type":"…","data":{…}}`,走 WebSocket text 帧。见 [`message`] / [`frame`]。
//! * **数据面信令流**(`docs/design-dataplane.md`):`[u32 BE len][CBOR]`,走 yamux
//!   stream 1。见 [`signal`]。

pub mod frame;
pub mod message;
pub mod signal;
pub mod stream;

pub use frame::{decode, encode, CodecError};
// 供 agent / server 直接给 iroh 寻址类型命名,不必自己依赖 `iroh-base`。
pub use iroh_base::{EndpointAddr, EndpointId, PublicKey, Signature};
pub use message::{
    login_message, random_subdomain, validate_subdomain, AuthChallenge, AuthError, AuthMethod,
    AuthOk, AuthResponse, ClientHello, ClientKind, Frame, P2PAddrResp, P2PAnnounce,
    P2PAnnounceResp, P2PList, P2PListResp, P2PLookup, ServerHello, TcpMapped, TokenCheck,
    TokenCheckResp, TrafficReport, TunnelClose, TunnelKind, TunnelPublish, TunnelResp,
    TunnelResult, LOGIN_DOMAIN, MAX_MESSAGE_BYTES, SUBDOMAIN_LEN,
};
pub use stream::{read_service, write_service, MAX_SERVICE_LEN};
pub use signal::{
    AuthRequest, AuthResponse as SignalAuthResponse, AuthStatus, SignalCodec, SignalCodecError,
    SignalFrame, MAX_SIGNAL_FRAME,
};

/// Acceptor → dialer:流前导通过,本地服务已接上,可以开始发 payload。
pub const STREAM_ACK_OK: u8 = 0;
/// Acceptor → dialer:流被拒绝(凭据/服务名不对,或本地服务连不上),重试无用。
pub const STREAM_ACK_ERR: u8 = 1;
