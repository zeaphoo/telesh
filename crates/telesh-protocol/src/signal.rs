//! 数据面信令流(stream 1)的消息与分帧。
//!
//! agent 主动连上服务端的数据端口后立刻建立 yamux 会话,并打开第一条 stream
//! ——**信令流**:先做鉴权,之后只做保活。payload 走服务端开的其他 stream。
//!
//! 分帧用 `tokio-util` 的 [`LengthDelimitedCodec`](u32 BE 长度前缀,WS 之外没有
//! 自带分帧的层),payload 用 CBOR。见 `docs/design-dataplane.md` §4。

use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

/// 单条信令帧的字节上限。
pub const MAX_SIGNAL_FRAME: usize = 64 * 1024;

/// 信令流上的消息。`type` 在 CBOR payload 里,信封为 `{"type":…,"data":…}`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum SignalFrame {
    /// agent → server:鉴权。
    Auth(AuthRequest),
    /// server → agent:鉴权结果。
    AuthResp(AuthResponse),
    /// 双向:保活。
    Ping,
    /// 双向:保活应答。
    Pong,
}

/// agent → server:复用登录会话完成数据连接鉴权。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    /// 登录 `AuthOk` 里拿到的会话令牌。
    #[serde(with = "serde_bytes")]
    pub session: Vec<u8>,
    /// agent 侧 yamux 的 `max_num_streams`;服务端据此钳制自己开流的数量。
    pub agent_max_streams: u32,
}

/// server → agent:鉴权结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub status: AuthStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    Ok,
    /// 会话无效或已过期。
    BadSession,
    /// 该 agent 的数据连接数超限。
    TooManyConnections,
    /// 服务端内部错误。
    Internal,
}

/// 一帧信令消息。
///
/// 未知 `type` 必须能被忽略而不断开连接,所以解码结果区分"已知"和"未知"。
#[derive(Debug, Clone)]
pub enum SignalMessage {
    Known(SignalFrame),
    /// 不认识的 `type`:忽略该帧即可。
    Unknown,
}

/// 本实现认识的 `type`,用来判断一帧是否需要忽略。
const KNOWN_TYPES: [&str; 4] = ["auth", "auth_resp", "ping", "pong"];

#[derive(Debug, thiserror::Error)]
pub enum SignalCodecError {
    #[error("framing: {0}")]
    Framing(#[from] std::io::Error),
    #[error("cbor encode: {0}")]
    Encode(String),
    #[error("cbor decode: {0}")]
    Decode(String),
}

/// `SignalFrame` 的编解码:`[u32 BE len][CBOR payload]`。
#[derive(Debug, Clone)]
pub struct SignalCodec(LengthDelimitedCodec);

impl SignalCodec {
    pub fn new() -> Self {
        let mut builder = LengthDelimitedCodec::builder();
        builder
            .big_endian()
            .length_field_type::<u32>()
            .max_frame_length(MAX_SIGNAL_FRAME);
        Self(builder.new_codec())
    }
}

impl Default for SignalCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalCodec {
    /// 解一帧:先看 CBOR payload 里的 `type`,不认识就返回 [`SignalMessage::Unknown`]。
    fn parse(&self, frame: &[u8]) -> Result<SignalMessage, SignalCodecError> {
        let value: ciborium::value::Value = ciborium::from_reader(frame)
            .map_err(|e| SignalCodecError::Decode(e.to_string()))?;
        let kind = value.as_map().and_then(|entries| {
            entries.iter().find_map(|(k, v)| match (k, v) {
                (ciborium::value::Value::Text(k), ciborium::value::Value::Text(t))
                    if k == "type" =>
                {
                    Some(t.as_str())
                }
                _ => None,
            })
        });
        match kind {
            Some(kind) if KNOWN_TYPES.contains(&kind) => {
                let frame: SignalFrame = ciborium::from_reader(frame)
                    .map_err(|e| SignalCodecError::Decode(e.to_string()))?;
                Ok(SignalMessage::Known(frame))
            }
            // 未知 type(或根本没有 type):按协议忽略这一帧。
            _ => Ok(SignalMessage::Unknown),
        }
    }
}

impl Decoder for SignalCodec {
    type Item = SignalMessage;
    type Error = SignalCodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.0.decode(src)? {
            Some(frame) => self.parse(&frame).map(Some),
            None => Ok(None),
        }
    }

    /// 帧边界上收到 EOF 已经是连接结束;半条消息按框架错误上报。
    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.0.decode_eof(src)? {
            Some(frame) => self.parse(&frame).map(Some),
            None => Ok(None),
        }
    }
}

impl Encoder<SignalFrame> for SignalCodec {
    type Error = SignalCodecError;

    fn encode(&mut self, item: SignalFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let mut payload = Vec::new();
        ciborium::into_writer(&item, &mut payload)
            .map_err(|e| SignalCodecError::Encode(e.to_string()))?;
        self.0.encode(Bytes::from(payload), dst)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_roundtrip() {
        let cases = vec![
            SignalFrame::Ping,
            SignalFrame::Pong,
            SignalFrame::Auth(AuthRequest {
                session: vec![1, 2, 3, 4],
                agent_max_streams: 512,
            }),
            SignalFrame::AuthResp(AuthResponse {
                status: AuthStatus::BadSession,
            }),
        ];
        for case in cases {
            let mut codec = SignalCodec::new();
            let mut buf = BytesMut::new();
            codec.encode(case.clone(), &mut buf).unwrap();
            // 长度前缀是大端 u32,并且确实位于 CBOR 之前
            let declared = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
            assert_eq!(declared, buf.len() - 4);
            let decoded = codec.decode(&mut buf).unwrap().unwrap();
            assert_eq!(format!("{decoded:?}"), format!("{:?}", SignalMessage::Known(case)));
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn unknown_type_is_ignored_not_an_error() {
        // 将来对端加消息时,老实现必须能跳过它
        let payload = ciborium::value::Value::Map(vec![(
            ciborium::value::Value::Text("type".into()),
            ciborium::value::Value::Text("some_future_message".into()),
        )]);
        let mut body = Vec::new();
        ciborium::into_writer(&payload, &mut body).unwrap();
        let mut codec = SignalCodec::new();
        let mut buf = BytesMut::new();
        codec
            .encode(SignalFrame::Ping, &mut buf)
            .unwrap(); // 先放一帧,保证 framing 状态正常
        buf.clear();
        LengthDelimitedCodec::builder()
            .big_endian()
            .length_field_type::<u32>()
            .max_frame_length(MAX_SIGNAL_FRAME)
            .new_codec()
            .encode(Bytes::from(body), &mut buf)
            .unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert!(matches!(decoded, SignalMessage::Unknown));
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut codec = SignalCodec::new();
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&u32::MAX.to_be_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        assert!(codec.decode(&mut buf).is_err());
    }
}
