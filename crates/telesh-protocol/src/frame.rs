//! Length-prefixed framing for [`Frame`] messages.
//!
//! Wire format: `[u32 big-endian body length][bincode body]`. A single
//! WebSocket binary message carries exactly one frame.

use crate::message::{Frame, MAX_FRAME_BODY};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("frame body length {0} exceeds maximum {MAX_FRAME_BODY}")]
    BodyTooLarge(usize),
    #[error("message too short to contain a length prefix")]
    Truncated,
    #[error("frame body length {0} does not match message size {1}")]
    LengthMismatch(usize, usize),
    #[error("bincode: {0}")]
    Bincode(#[from] bincode::Error),
}

/// Serialize a frame into a length-prefixed wire message.
pub fn encode(frame: &Frame) -> Result<Vec<u8>, CodecError> {
    let body = bincode::serialize(frame)?;
    if body.len() > MAX_FRAME_BODY {
        return Err(CodecError::BodyTooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode exactly one frame from a buffer that should contain a single
/// complete message (the common case: one WebSocket binary message).
pub fn decode(msg: &[u8]) -> Result<Frame, CodecError> {
    if msg.len() < 4 {
        return Err(CodecError::Truncated);
    }
    let len = u32::from_be_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    if len > MAX_FRAME_BODY {
        return Err(CodecError::BodyTooLarge(len));
    }
    if len != msg.len() - 4 {
        return Err(CodecError::LengthMismatch(len, msg.len() - 4));
    }
    Ok(bincode::deserialize(&msg[4..])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{
        AuthChallenge, AuthError, AuthMethod, AuthOk, AuthResponse, ClientHello, ClientKind,
        P2PAddrResp, P2PAnnounce, P2PAnnounceResp, P2PLookup, ServerHello, StreamPacket, TcpMapped,
        TokenCheck, TokenCheckResp, TrafficReport, LookupHello,
    };

    #[test]
    fn roundtrip_small_frames() {
        let frames = [
            Frame::Hello(ClientHello {
                protocol_version: 2,
                client_kind: ClientKind::Agent,
                method: AuthMethod::Keypair,
                public_key: Some([7u8; 32]),
                session: None,
                enrollment: Some("enroll".into()),
                sub_domain: Some("web".into()),
                display_name: Some("Calm Otter".into()),
            }),
            Frame::AuthChallenge(AuthChallenge {
                nonce: [1u8; 32],
                server_id: [2u8; 32],
                ts: 1234,
            }),
            Frame::AuthResponse(AuthResponse {
                signature: vec![3u8; 64],
            }),
            Frame::AuthOk(AuthOk {
                account: "000012345".into(),
                session: vec![9, 9, 9],
                expires_at: 9999,
                display_name: "Calm Otter".into(),
            }),
            Frame::AuthError(AuthError::BadSignature),
            Frame::TokenCheck(TokenCheck {
                id: 7,
                account: "000012345".into(),
                name: "svc".into(),
                token: vec![5, 5, 5],
            }),
            Frame::TokenCheckResp(TokenCheckResp {
                id: 7,
                ok: true,
                expires_at: 1234,
            }),
            Frame::ServerHello(ServerHello::Success {
                sub_domain: "web".into(),
                hostname: "web.example.com".into(),
                agent_id: "000012345".into(),
            }),
            Frame::stream(StreamPacket::Init { stream_id: 42 }),
            Frame::stream(StreamPacket::Data {
                stream_id: 42,
                data: vec![0xde, 0xad, 0xbe, 0xef],
            }),
            Frame::Ping,
            Frame::Pong,
        ];
        for f in frames {
            let wire = encode(&f).unwrap();
            let decoded = decode(&wire).unwrap();
            assert_eq!(
                format!("{:?}", decoded),
                format!("{:?}", f),
                "frame mismatch"
            );
        }
    }

    #[test]
    fn roundtrip_p2p_frames() {
        let id = iroh_base::SecretKey::generate().public();
        let addr = iroh_base::EndpointAddr::new(id);
        let frames = [
            Frame::P2PAnnounce(P2PAnnounce {
                name: "svc".into(),
                addr: addr.clone(),
                local_port: 8000,
            }),
            Frame::P2PLookup(P2PLookup {
                account: "000012345".into(),
                name: "svc".into(),
            }),
            Frame::P2PAddr(P2PAddrResp::Found {
                account: "000012345".into(),
                name: "svc".into(),
                owner_name: "Calm Otter".into(),
                addr: addr.clone(),
                token: Some(vec![1, 2, 3]),
            }),
            Frame::P2PAddr(P2PAddrResp::NotFound),
            Frame::Lookup(LookupHello {
                protocol_version: 1,
                auth_key: Some("secret".into()),
                agent_id: "peer-1".into(),
            }),
            Frame::IrohReady(addr.clone()),
            Frame::Traffic(TrafficReport {
                name: "svc".into(),
                up: 1024,
                down: 2048,
            }),
            Frame::TcpMapped(TcpMapped {
                name: "svc".into(),
                addr: "203.0.113.7:22000".parse().unwrap(),
                local_port: 22,
            }),
            Frame::P2PAnnounceResp(P2PAnnounceResp::Ok {
                name: "svc".into(),
            }),
            Frame::P2PAnnounceResp(P2PAnnounceResp::NameTaken {
                name: "svc".into(),
            }),
            Frame::P2PAnnounceResp(P2PAnnounceResp::InvalidName {
                name: "BAD".into(),
            }),
        ];
        for f in frames {
            let wire = encode(&f).unwrap();
            let decoded = decode(&wire).unwrap();
            assert_eq!(
                format!("{:?}", decoded),
                format!("{:?}", f),
                "frame mismatch"
            );
        }
    }

    #[test]
    fn rejects_oversized_body() {
        let mut buf = vec![0u8; 4];
        buf[0] = 0x02; // 32MB > MAX_FRAME_BODY
        assert!(matches!(decode(&buf), Err(CodecError::BodyTooLarge(_))));
    }

    #[test]
    fn rejects_length_mismatch() {
        let mut buf = vec![0u8; 8];
        buf[0] = 0x00;
        buf[1] = 0x00;
        buf[2] = 0x00;
        buf[3] = 0x10; // claims 16 bytes but only 4 present
        assert!(matches!(decode(&buf), Err(CodecError::LengthMismatch(16, 4))));
    }
}