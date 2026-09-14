//! Semantic wire messages exchanged between a telesh agent and the telesh server.
//!
//! The control plane is a single long-lived WebSocket connection. All messages
//! are serialized with `bincode` and framed with a 4-byte big-endian length
//! prefix (see [`crate::frame`]).
//!
//! Data plane: individual TCP streams are multiplexed over the same control
//! connection using [`StreamPacket`] frames carrying a `stream_id`. This keeps
//! the MVP simple; a dedicated data-plane transport (bore-style reverse
//! connections) can be introduced later without changing the message types.

use serde::{Deserialize, Serialize};

/// Protocol version this implementation speaks. Bumped on breaking wire changes.
pub const PROTOCOL_VERSION: u16 = 1;

/// Random subdomain length when the agent does not request a specific one.
pub const SUBDOMAIN_LEN: usize = 8;

/// Hard cap on one decoded frame payload. Data frames can be large (a full TCP
/// read burst), but this bounds memory per frame against malicious peers.
pub const MAX_FRAME_BODY: usize = 16 * 1024 * 1024;

/// Agent -> server: request a TCP tunnel port, e.g. for SSH/NAS/raw TCP.
/// A connection may own many TCP tunnels; the server allocates one port per
/// request and answers with `TcpRegisterResp`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpRegister {
    /// Preferred remote port; `None` asks the server to pick one.
    pub requested_port: Option<u16>,
}

/// Outcome of a [`TcpRegister`] request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TcpRegisterResp {
    Success {
        /// Allocated public TCP port on the server.
        port: u16,
    },
    PortInUse,
    /// Port outside the server's configured TCP range.
    InvalidPort,
    Error(String),
}

/// A message sent over the wire.
///
/// Bincode encodes enums by variant index, so adding variants in the middle
/// breaks compatibility; append new variants at the end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Frame {
    /// Agent -> server, always the first message on a fresh connection.
    Hello(ClientHello),
    /// Server -> agent, reply to `Hello`.
    ServerHello(ServerHello),
    /// Data-plane traffic for a multiplexed stream.
    Stream(StreamPacket),
    /// Server -> agent keepalive probe.
    Ping,
    /// Agent -> server keepalive response.
    Pong,
    /// Agent -> server: request a TCP tunnel port.
    TcpRegister(TcpRegister),
    /// Server -> agent: result of a `TcpRegister` request.
    TcpRegisterResp(TcpRegisterResp),
}

/// Agent identity details sent on connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol_version: u16,
    /// Stable identity persisted by the agent across reconnects. The server
    /// replaces any existing connection with the same `agent_id`, enabling
    /// seamless re-registration after a dropped control connection.
    pub agent_id: String,
    /// Requested subdomain. `None` -> server assigns a random one.
    pub sub_domain: Option<String>,
    /// Secret for servers configured with a master key. Optional; the server
    /// rejects the connection when a key is required but missing/wrong.
    pub auth_key: Option<String>,
}

/// Outcome of the handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerHello {
    Success {
        sub_domain: String,
        /// Public hostname of the tunnel, e.g. `<sub>.<public-host>`.
        hostname: String,
        agent_id: String,
    },
    SubDomainInUse,
    InvalidSubDomain,
    AuthFailed,
    Error(String),
}

/// Multiplexed stream traffic over the control connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamPacket {
    /// Server -> agent: an incoming public connection with `stream_id` arrived;
    /// agent should dial the local service and start pumping bytes.
    Init {
        stream_id: u64,
    },
    /// Either direction: raw bytes for `stream_id`.
    Data {
        stream_id: u64,
        data: Vec<u8>,
    },
    /// Either direction: stream closed cleanly.
    End {
        stream_id: u64,
    },
    /// Agent -> server: local dial failed, reject the stream.
    Refused {
        stream_id: u64,
    },
}

impl Frame {
    pub fn stream(packet: StreamPacket) -> Self {
        Frame::Stream(packet)
    }
}