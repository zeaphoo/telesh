//! Embedded relay configuration file.
//!
//! Used only when the server is started with `--relay-serve <addr>`. The file
//! is read from `$TELESH_RELAY_CONFIG`, defaulting to `~/.telesh/relay.toml`.
//! If it is missing, the relay runs over plain HTTP.
//!
//! ```toml
//! # Manual TLS (HTTPS + QUIC address discovery):
//! tls_cert   = "/etc/telesh/relay.crt"
//! tls_key    = "/etc/telesh/relay.key"
//! https_addr = "0.0.0.0:443"
//! quic_addr  = "0.0.0.0:7842"
//!
//! # ...or ACME/Let's Encrypt (takes precedence over manual TLS):
//! acme         = ["relay.example.com"]
//! acme_email   = "admin@example.com"
//! acme_staging = false
//! acme_cache   = "/var/lib/telesh/acme"
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RelayConfigFile {
    /// PEM certificate chain; with `tls_key` enables HTTPS + QUIC.
    pub tls_cert: Option<PathBuf>,
    /// PEM private key for `tls_cert`.
    pub tls_key: Option<PathBuf>,
    /// HTTPS bind address (required with TLS/ACME; usually port 443).
    pub https_addr: Option<SocketAddr>,
    /// QUIC address-discovery bind address (default `0.0.0.0:7842`).
    pub quic_addr: Option<SocketAddr>,
    /// ACME domains; non-empty enables Let's Encrypt and takes precedence.
    pub acme: Vec<String>,
    /// ACME account contact (uses `mailto:` for an email).
    pub acme_email: Option<String>,
    /// Use the Let's Encrypt staging directory.
    pub acme_staging: bool,
    /// Directory to cache ACME certificates in.
    pub acme_cache: Option<PathBuf>,
}

/// Load the relay config file, or an empty config when it does not exist.
pub fn load() -> Result<RelayConfigFile> {
    let path = std::env::var("TELESH_RELAY_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(format!("{home}/.telesh/relay.toml"))
        });
    if !path.exists() {
        return Ok(RelayConfigFile::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read relay config {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("cannot parse relay config {}", path.display()))
}

impl RelayConfigFile {
    /// QUIC bind address, with the customary default.
    pub fn quic_addr_or_default(&self) -> SocketAddr {
        self.quic_addr
            .unwrap_or_else(|| "0.0.0.0:7842".parse().expect("valid default"))
    }
}
