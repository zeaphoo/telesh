//! telesh-protocol — wire protocol shared by the telesh agent and server.
//!
//! A single long-lived WebSocket control connection carries both control
//! messages and multiplexed stream data (see [`message`] and [`frame`]).

pub mod frame;
pub mod message;

pub use frame::{decode, encode, CodecError};
pub use message::{
    ClientHello, Frame, ServerHello, StreamPacket, TcpRegister, TcpRegisterResp, PROTOCOL_VERSION,
    SUBDOMAIN_LEN,
};

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