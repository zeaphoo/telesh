//! 本机 agent 的 HTTP 客户端。
//!
//! 只和 `127.0.0.1:<ui-port>` 说话,不需要 TLS。用最小的 HTTP/1.0 客户端,
//! 避免为了一个本机请求把 reqwest/rustls 带进 agent 的构建。

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 一个最小的、只面向本机 agent 的 HTTP 客户端。
pub(crate) struct AgentClient {
    addr: SocketAddr,
}

impl AgentClient {
    /// 连接并确认端口后面真的是 telesh(与 `instance.rs`、桌面外壳同一指纹)。
    pub(crate) async fn connect(ui_port: u16) -> Result<Self> {
        let client = Self {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, ui_port)),
        };

        let instance: crate::instance::Instance =
            client.get("/api/instance").await.with_context(|| {
                format!(
                    "无法确认 http://127.0.0.1:{ui_port} 上是 telesh;面板没在跑(先运行 `telesh`,或用 systemd 启动),或端口上是别的服务(可用 --ui-port 指定)"
                )
            })?;
        if instance.app != "telesh" {
            bail!("http://127.0.0.1:{ui_port} 上不是 telesh,拒绝操作(可用 --ui-port 指定别的端口)");
        }
        Ok(client)
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let body = self.request("GET", path, None).await?;
        serde_json::from_slice(&body).with_context(|| format!("{path}: 返回的不是合法 JSON"))
    }

    pub(crate) async fn put<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        payload: &B,
    ) -> Result<T> {
        let body = serde_json::to_vec(payload)?;
        let body = self.request("PUT", path, Some(body)).await?;
        serde_json::from_slice(&body).with_context(|| format!("{path}: 返回的不是合法 JSON"))
    }

    async fn request(&self, method: &str, path: &str, body: Option<Vec<u8>>) -> Result<Vec<u8>> {
        let fut = async {
            let mut stream = tokio::net::TcpStream::connect(self.addr).await?;
            // HTTP/1.0 + `Connection: close`:响应不会被 chunked,读到底就是 body。
            let mut head =
                format!("{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
            if let Some(body) = &body {
                head.push_str("Content-Type: application/json\r\n");
                head.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            head.push_str("\r\n");
            stream.write_all(head.as_bytes()).await?;
            if let Some(body) = body {
                stream.write_all(&body).await?;
            }

            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await?;
            parse_http_response(&raw)
        };

        let (status, body) = tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .map_err(|_| anyhow::anyhow!("{method} {path}: 请求超时"))??;
        if !(200..300).contains(&status) {
            bail!(
                "{method} {path}: HTTP {status}: {}",
                String::from_utf8_lossy(&body).trim()
            );
        }
        Ok(body)
    }
}

/// 拆出状态码与 body。只处理 `Content-Length` / 连接关闭这类简单响应 ——
/// agent 的 JSON API 都是这种。
fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>)> {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("agent 返回了非 HTTP 响应")?;
    let head = std::str::from_utf8(&raw[..sep]).context("agent 返回了非 UTF-8 响应头")?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .context("agent 返回了无法解析的状态行")?;
    Ok((status, raw[sep + 4..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_and_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"{}");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_http_response(b"not http").is_err());
    }
}
