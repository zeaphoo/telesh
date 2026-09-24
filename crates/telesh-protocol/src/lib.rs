//! telesh-protocol — wire protocol shared by the telesh agent and server.
//!
//! A single long-lived WebSocket control connection carries both control
//! messages and multiplexed stream data (see [`message`] and [`frame`]).

pub mod frame;
pub mod message;

pub use frame::{decode, encode, CodecError};
// Re-exported so the agent and server can name the P2P addressing type without
// depending on `iroh-base` directly.
pub use iroh_base::{EndpointAddr, EndpointId, PublicKey, Signature};
pub use message::{
    login_message, AuthChallenge, AuthError, AuthMethod, AuthOk, AuthResponse, ClientHello,
    ClientKind, Frame, P2PAddrResp, P2PAnnounce, P2PAnnounceResp, P2PLookup, ServerHello,
    StreamPacket, TcpMapped, TcpRegister, TcpRegisterResp, TokenCheck, TokenCheckResp,
    TrafficReport, LookupHello, LOGIN_DOMAIN, PROTOCOL_VERSION, SUBDOMAIN_LEN,
};

/// Acceptor -> dialer: the stream preamble was accepted and the local service
/// is attached; payload may flow.
pub const STREAM_ACK_OK: u8 = 0;
/// Acceptor -> dialer: the stream was refused (bad credential/service, or the
/// local service could not be dialed). Retrying the same stream will not help.
pub const STREAM_ACK_ERR: u8 = 1;

/// Random alphanumeric subdomain used when the agent does not request one.
/// Mirrors the character set used by ngrok-style services.
pub fn random_subdomain() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..SUBDOMAIN_LEN)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// Validate a user-requested subdomain: `[a-z0-9-]`, 1..=63 chars.
pub fn validate_subdomain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}