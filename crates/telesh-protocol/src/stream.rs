//! 流首的"目标字段":长度前缀的 UTF-8 目标 / 服务名。
//!
//! iroh 路径的流前导和数据面的 payload 流共用这一套编码:
//!
//! ```text
//! [u16 BE len][len 字节 UTF-8]
//! ```
//!
//! 数据面上 dialer 是已鉴权的 edge,所以只有目标字段本身,没有凭据和 ack;
//! iroh 前导在它前面还有凭据。见 `docs/design-dataplane.md` §8。

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// 目标名的字节上限(和 iroh 前导的 `MAX_PRELUDE` 一致)。
pub const MAX_SERVICE_LEN: usize = 256;

/// 写一个长度前缀的目标字段。
pub async fn write_service<W: AsyncWrite + Unpin>(
    w: &mut W,
    service: &str,
) -> std::io::Result<()> {
    let bytes = service.as_bytes();
    if bytes.len() > MAX_SERVICE_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("service name too long ({} bytes)", bytes.len()),
        ));
    }
    w.write_all(&(bytes.len() as u16).to_be_bytes()).await?;
    w.write_all(bytes).await?;
    Ok(())
}

/// 读一个长度前缀的目标字段。
pub async fn read_service<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<String> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len).await?;
    let len = u16::from_be_bytes(len) as usize;
    if len > MAX_SERVICE_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("service name too long ({len} bytes)"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn service_roundtrip() {
        let mut buf = Vec::new();
        write_service(&mut buf, "8000").await.unwrap();
        assert_eq!(buf, vec![0x00, 0x04, b'8', b'0', b'0', b'0']);
        let mut cursor = &buf[..];
        assert_eq!(read_service(&mut cursor).await.unwrap(), "8000");
    }

    #[tokio::test]
    async fn oversized_service_is_rejected() {
        let long = "x".repeat(MAX_SERVICE_LEN + 1);
        assert!(write_service(&mut Vec::new(), &long).await.is_err());
    }
}
