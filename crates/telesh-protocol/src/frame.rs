//! 控制面编解码:JSON 文本,信封 `{"type":"…","data":{…}}`。
//!
//! WebSocket 自带消息边界,所以**没有**长度前缀。见 `docs/design-control.md` §2。

use crate::message::{Frame, MAX_MESSAGE_BYTES};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("message of {0} bytes exceeds the {MAX_MESSAGE_BYTES} byte limit")]
    TooLarge(usize),
}

/// 把一条控制消息编码成 JSON 文本。
pub fn encode(frame: &Frame) -> Result<String, CodecError> {
    let text = serde_json::to_string(frame)?;
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(CodecError::TooLarge(text.len()));
    }
    Ok(text)
}

/// 解码一条 JSON 控制消息。
///
/// 未知 `type` 会返回 `Err`;调用方应当**记录并忽略**它,而不是断开连接。
pub fn decode(text: &str) -> Result<Frame, CodecError> {
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(CodecError::TooLarge(text.len()));
    }
    Ok(serde_json::from_str(text)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{
        AuthMethod, AuthOk, ClientHello, ClientKind, P2PAnnounce, ServerHello, TunnelClose,
        TunnelKind, TunnelPublish, TunnelResp, TunnelResult,
    };
    use iroh_base::{EndpointAddr, SecretKey, TransportAddr};

    fn endpoint_addr() -> EndpointAddr {
        let key = SecretKey::generate();
        let mut addr = EndpointAddr::new(key.public());
        addr.addrs
            .insert(TransportAddr::Ip("192.168.1.5:1234".parse().unwrap()));
        addr.addrs
            .insert(TransportAddr::Relay("https://relay.example.com".parse().unwrap()));
        addr
    }

    #[test]
    fn envelope_shape() {
        assert_eq!(encode(&Frame::Ping).unwrap(), r#"{"type":"ping"}"#);
        assert_eq!(encode(&Frame::Pong).unwrap(), r#"{"type":"pong"}"#);
    }

    #[test]
    fn control_roundtrip() {
        let frames = vec![
            Frame::Ping,
            Frame::Pong,
            Frame::Hello(ClientHello {
                client_kind: ClientKind::Agent,
                method: AuthMethod::Keypair,
                public_key: Some([7u8; 32]),
                session: Some(vec![1, 2, 3, 4]),
                enrollment: Some("enroll".into()),
                display_name: Some("Calm Otter".into()),
            }),
            Frame::AuthOk(AuthOk {
                account: "000012345".into(),
                session: vec![9, 9, 9],
                expires_at: 9999,
                display_name: "Calm Otter".into(),
            }),
            Frame::ServerHello(ServerHello::Success {
                agent_id: "000012345".into(),
                relay_url: Some("https://relay.example.com/".into()),
                data_addr: Some("203.0.113.7:9008".into()),
            }),
            Frame::TunnelPublish(TunnelPublish {
                id: 7,
                kind: TunnelKind::Http,
                target: "8000".into(),
                name: Some("web".into()),
                remote_port: None,
            }),
            Frame::TunnelResp(TunnelResp {
                id: 7,
                result: TunnelResult::Http {
                    sub_domain: "web".into(),
                    hostname: "web.example.com".into(),
                },
            }),
            Frame::TunnelClose(TunnelClose {
                kind: TunnelKind::Http,
                key: "web".into(),
            }),
            Frame::IrohReady(endpoint_addr()),
            Frame::P2PAnnounce(P2PAnnounce {
                name: "svc".into(),
                addr: endpoint_addr(),
                local_port: 8000,
            }),
        ];
        for f in frames {
            let text = encode(&f).unwrap();
            let back = decode(&text).unwrap();
            assert_eq!(format!("{back:?}"), format!("{f:?}"), "roundtrip: {text}");
        }
    }

    #[test]
    fn endpoint_id_is_hex_and_addrs_carry_a_kind_prefix() {
        let addr = endpoint_addr();
        let text = encode(&Frame::IrohReady(addr.clone())).unwrap();
        assert!(text.contains(&format!("\"id\":\"{}\"", addr.id)), "{text}");
        assert!(text.contains("\"ip:192.168.1.5:1234\""), "{text}");
        assert!(text.contains("\"relay:https://relay.example.com/\""), "{text}");

        let back = decode(&text).unwrap();
        assert_eq!(format!("{back:?}"), format!("{:?}", Frame::IrohReady(addr)));
    }

    #[test]
    fn unknown_type_is_an_error_the_caller_ignores() {
        assert!(decode(r#"{"type":"not_a_message"}"#).is_err());
    }
}
