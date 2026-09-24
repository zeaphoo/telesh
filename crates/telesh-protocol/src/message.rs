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

use iroh_base::EndpointAddr;
use serde::{Deserialize, Serialize};

/// Protocol version this implementation speaks. Bumped on breaking wire changes.
pub const PROTOCOL_VERSION: u16 = 2;

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
    /// Agent -> server: register a service peers reach directly over iroh.
    P2PAnnounce(P2PAnnounce),
    /// Peer -> server: request the `EndpointAddr` for `account/name`.
    P2PLookup(P2PLookup),
    /// Server -> peer: result of a `P2PLookup`.
    P2PAddr(P2PAddrResp),
    /// Peer -> server, always the first message on a lookup-only connection.
    Lookup(LookupHello),
    /// Agent -> server: the agent's iroh endpoint for the direct data plane.
    /// Sent (and re-sent when the address changes) by http/tcp agents so the
    /// edge can route ingress traffic peer-to-peer, with the WebSocket relay as
    /// fallback.
    IrohReady(EndpointAddr),
    /// Agent -> server: bytes moved over the peer-to-peer data plane, which the
    /// server cannot observe itself.
    Traffic(TrafficReport),
    /// Agent -> server: the public TCP endpoint obtained via router port mapping.
    TcpMapped(TcpMapped),
    /// Server -> agent: result of one `P2PAnnounce` (name ownership).
    P2PAnnounceResp(P2PAnnounceResp),
    /// Server -> client: login challenge to sign.
    AuthChallenge(AuthChallenge),
    /// Client -> server: signature over the challenge.
    AuthResponse(AuthResponse),
    /// Server -> client: login succeeded.
    AuthOk(AuthOk),
    /// Server -> client: login failed.
    AuthError(AuthError),
    /// Agent -> server: is this opaque token valid for `account/name`?
    TokenCheck(TokenCheck),
    /// Server -> agent: answer to `TokenCheck`.
    TokenCheckResp(TokenCheckResp),
}

/// What kind of client is connecting.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClientKind {
    Agent,
    Lookup,
}

/// How a client authenticates. Only keypair is implemented; password/OIDC/
/// real-name are planned.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMethod {
    Keypair,
}

/// Domain separator for the login signature.
pub const LOGIN_DOMAIN: &[u8] = b"telesh-login-v1";

/// Message a client signs to prove it holds the private key for its EndpointId.
/// It is domain-separated and bound to `server_id` so it cannot be replayed on
/// another server.
pub fn login_message(server_id: &[u8; 32], nonce: &[u8; 32], ts: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(LOGIN_DOMAIN.len() + 32 + 32 + 8);
    m.extend_from_slice(LOGIN_DOMAIN);
    m.extend_from_slice(server_id);
    m.extend_from_slice(nonce);
    m.extend_from_slice(&ts.to_be_bytes());
    m
}

/// Server -> client: sign this to log in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub nonce: [u8; 32],
    /// Stable server identifier, mixed into the signed message.
    pub server_id: [u8; 32],
    /// Unix seconds, bounds the challenge's validity.
    pub ts: u64,
}

/// Client -> server: proof of possession of the private key.
///
/// The signature is a `Vec<u8>` (64 bytes) because serde does not implement
/// traits for `[u8; 64]`; the receiver checks the length.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub signature: Vec<u8>,
}

/// Server -> client: authenticated; here is your session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthOk {
    /// 9-digit account handle, zero-padded (e.g. `"000012345"`).
    pub account: String,
    /// Opaque session token to present on later connections.
    pub session: Vec<u8>,
    /// Session expiry (unix seconds).
    pub expires_at: u64,
    /// The account's friendly node name as stored by the server.
    pub display_name: String,
}

/// Agent -> server: validate an opaque access token a peer presented.
///
/// The server is the only holder of the token store, so validation is on-line
/// (no server signing key, no offline signature check).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCheck {
    /// Correlates the reply with this request.
    pub id: u64,
    pub account: String,
    pub name: String,
    pub token: Vec<u8>,
}

/// Server -> agent: result of a `TokenCheck`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCheckResp {
    pub id: u64,
    pub ok: bool,
    /// Token expiry (unix seconds) when `ok`; the agent caches the grant for the
    /// rest of the iroh connection until then.
    pub expires_at: u64,
}

/// Server -> client: authentication failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthError {
    BadSignature,
    BadSession,
    SessionExpired,
    EnrollmentRequired,
    EnrollmentRejected,
    UnsupportedMethod,
    Error(String),
}

/// Server -> agent: outcome of a `P2PAnnounce` for one service name.
///
/// A service name is owned by the `EndpointId` that first registers it, so a
/// different agent can never silently take it over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum P2PAnnounceResp {
    /// The name is now owned by this endpoint.
    Ok { name: String },
    /// A different endpoint already owns this name.
    NameTaken { name: String },
    /// The name is not a valid service name.
    InvalidName { name: String },
}

/// Agent -> server: periodic counters for traffic the server cannot see.
///
/// On the pure P2P path (`telesh connect`) neither the edge nor the control
/// connection carries payload, so only the agent can account for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficReport {
    /// Service name the counters belong to.
    pub name: String,
    /// Bytes sent to peers.
    pub up: u64,
    /// Bytes received from peers.
    pub down: u64,
}

/// Agent -> server: a public TCP endpoint opened by router port mapping
/// (UPnP/PCP/NAT-PMP), independent of iroh and of the server's ingress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpMapped {
    pub name: String,
    /// Public `IP:port` reported by the gateway.
    pub addr: std::net::SocketAddr,
    /// The local port that was mapped.
    pub local_port: u16,
}

/// Peer -> server: opens a lookup-only control connection.
///
/// Unlike [`ClientHello`], a peer registers no tunnel and claims no
/// subdomain; the server only answers its `P2PLookup` requests. This keeps the
/// registry free of throwaway entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupHello {
    pub protocol_version: u16,
    /// Secret for servers configured with a master key. Optional.
    pub auth_key: Option<String>,
    /// Ephemeral id, used for logging only.
    pub agent_id: String,
}

/// Agent -> server: register a service that peers can reach directly over iroh.
///
/// The server only stores the [`EndpointAddr`] and hands it to peers; in P2P
/// mode payload never flows through the server. The address carries the
/// endpoint id plus relay/direct addresses, so peers need no other discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PAnnounce {
    /// Stable service name used by peers to look the endpoint up.
    pub name: String,
    /// iroh addressing info (endpoint id + relay/direct addresses).
    pub addr: EndpointAddr,
    /// Local port being exposed, for the dashboard/UX only.
    pub local_port: u16,
}

/// Peer -> server: look up a service inside a specific account.
///
/// Service names are namespaced per account, so the peer must name both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PLookup {
    /// 9-digit account handle (zero-padded).
    pub account: String,
    /// Service name within that account.
    pub name: String,
}

/// Result of looking up an account-namespaced P2P service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum P2PAddrResp {
    Found {
        account: String,
        name: String,
        /// Friendly name of the owning node, for display/logging.
        owner_name: String,
        addr: EndpointAddr,
        /// Short-lived opaque access token for `account/name`, which the peer
        /// presents to the agent (the agent asks the server to validate it).
        token: Option<Vec<u8>>,
    },
    NotFound,
}

/// First client message: negotiate identity and authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol_version: u16,
    pub client_kind: ClientKind,
    pub method: AuthMethod,
    /// EndpointId (Ed25519 public key) for keypair login.
    pub public_key: Option<[u8; 32]>,
    /// Opaque session token for the fast path (skips the challenge).
    pub session: Option<Vec<u8>>,
    /// Required on first registration when the server has an enrollment secret.
    pub enrollment: Option<String>,
    /// Requested subdomain (agents only). `None` -> server assigns one.
    pub sub_domain: Option<String>,
    /// Friendly node name (LocalSend-style) the server shows to peers.
    pub display_name: Option<String>,
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