//! Agent login: keypair challenge-response against the control server.
//!
//! A cached session token is tried first (fast path); otherwise the server
//! sends a challenge that we sign with the agent's Ed25519 key.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures::{SinkExt, StreamExt};
use iroh::SecretKey;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use telesh_protocol::{
    decode, encode, login_message, AuthMethod, AuthResponse, ClientHello, ClientKind, Frame,
    ServerHello, PROTOCOL_VERSION,
};

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// A successful login.
pub struct Login {
    /// 9-digit account handle (zero-padded).
    pub account: String,
    /// Session token to cache and present on later connections.
    pub session: Vec<u8>,
    /// Session expiry (unix seconds).
    pub expires_at: u64,
    /// Friendly node name as stored by the server.
    pub display_name: String,
}

/// Log in over `ws`; returns the login result and the tunnel `ServerHello`.
///
/// The caller keeps ownership of `ws` (it is only borrowed) so it can split it
/// afterwards for the normal frame pump.
pub async fn login(
    ws: &mut WsStream,
    secret_key: &[u8; 32],
    session: Option<Vec<u8>>,
    enrollment: Option<&str>,
    sub_domain: Option<String>,
    display_name: Option<String>,
) -> Result<(Login, ServerHello)> {
    let sk = SecretKey::from_bytes(secret_key);
    let hello = ClientHello {
        protocol_version: PROTOCOL_VERSION,
        client_kind: ClientKind::Agent,
        method: AuthMethod::Keypair,
        public_key: Some(*sk.public().as_bytes()),
        session,
        enrollment: enrollment.map(str::to_string),
        sub_domain,
        display_name,
    };
    ws.send(WsMessage::Binary(encode(&Frame::Hello(hello))?))
        .await?;

    let mut login: Option<Login> = None;
    let mut server_hello: Option<ServerHello> = None;
    while server_hello.is_none() {
        let msg = tokio::time::timeout(Duration::from_secs(15), ws.next())
            .await
            .context("timed out during login")?
            .context("control connection closed during login")?
            .context("control connection error during login")?;
        let data = match msg {
            WsMessage::Binary(data) => data,
            WsMessage::Close(_) => bail!("control server closed during login"),
            _ => continue,
        };
        match decode(&data)? {
            Frame::AuthChallenge(challenge) => {
                let message = login_message(&challenge.server_id, &challenge.nonce, challenge.ts);
                let signature = sk.sign(&message).to_bytes().to_vec();
                ws.send(WsMessage::Binary(encode(&Frame::AuthResponse(
                    AuthResponse { signature },
                ))?))
                .await?;
            }
            Frame::AuthOk(ok) => {
                login = Some(Login {
                    account: ok.account,
                    session: ok.session,
                    expires_at: ok.expires_at,
                    display_name: ok.display_name,
                });
            }
            Frame::AuthError(err) => bail!("login rejected: {err:?}"),
            Frame::ServerHello(hello) => {
                match &hello {
                    ServerHello::SubDomainInUse
                    | ServerHello::InvalidSubDomain
                    | ServerHello::AuthFailed
                    | ServerHello::Error(_) => bail!("server rejected tunnel: {hello:?}"),
                    ServerHello::Success { .. } => {}
                }
                server_hello = Some(hello);
            }
            Frame::Ping => {
                ws.send(WsMessage::Binary(encode(&Frame::Pong)?)).await?;
            }
            _ => {}
        }
    }

    let Some(login) = login else {
        bail!("login did not complete (no AuthOk)");
    };
    Ok((login, server_hello.expect("loop exited with a ServerHello")))
}
