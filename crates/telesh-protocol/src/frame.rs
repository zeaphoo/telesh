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
    use crate::message::{ClientHello, ServerHello, StreamPacket};

    #[test]
    fn roundtrip_small_frames() {
        let frames = [
            Frame::Hello(ClientHello {
                protocol_version: 1,
                agent_id: "agent-1".into(),
                sub_domain: Some("web".into()),
                auth_key: Some("secret".into()),
            }),
            Frame::ServerHello(ServerHello::Success {
                sub_domain: "web".into(),
                hostname: "web.example.com".into(),
                agent_id: "agent-1".into(),
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