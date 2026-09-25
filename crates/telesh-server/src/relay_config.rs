//! Embedded relay configuration file.
//!
//! Used only when the server is started with `--relay-serve <addr>`. The file
//! is read from `$TELESH_RELAY_CONFIG`, defaulting to `~/.telesh/relay.toml`.
//! If it is missing, the relay runs over plain HTTP.
//!
//! ```toml
//! # Public URL announced to agents after login. Optional: with ACME it is
//! # derived from the first domain, with plain HTTP / manual TLS from
//! # --public-host. Set it when the relay is reachable under another name.
//! url = "https://relay.example.com"
//!
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

use crate::config::Config;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RelayConfigFile {
    /// Public URL this relay is reachable at, announced to agents after login.
    pub url: Option<String>,
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

    /// The public relay URL implied by this file alone, if any.
    ///
    /// An explicit `url` always wins. With ACME the first domain is the
    /// certificate identity, so it is the only usable host. Manual TLS and
    /// plain HTTP cannot know their host unless `--public-host` names it.
    fn derived_url(&self, cfg: &Config) -> Option<String> {
        if let Some(url) = self.url.as_deref().filter(|u| !u.is_empty()) {
            return Some(url.to_string());
        }
        // Nothing is derived unless we actually run the embedded relay.
        cfg.relay_serve?;
        if let Some(domain) = self.acme.first().filter(|d| !d.is_empty()) {
            let port = self.https_addr.map_or(443, |a| a.port());
            return Some(http_url("https", domain, port, 443));
        }
        let host = cfg.public_host.trim();
        if host.is_empty() {
            return None;
        }
        match self.https_addr {
            Some(addr) => Some(http_url("https", host, addr.port(), 443)),
            None => {
                let port = cfg.relay_serve.map_or(80, |a| a.port());
                Some(http_url("http", host, port, 80))
            }
        }
    }
}

/// The iroh relay URL the server announces to agents (and dials through
/// itself): `--relay-url`, else the relay config file, else the embedded
/// relay's derived URL, else the legacy `TELESH_RELAY` environment variable.
/// `None` leaves clients on iroh's default (n0) relays.
pub fn advertised_url(cfg: &Config, file: &RelayConfigFile) -> Option<String> {
    if let Some(url) = non_empty(&cfg.relay_url) {
        return Some(url);
    }
    if let Some(url) = file.derived_url(cfg) {
        return Some(url);
    }
    if let Some(url) = std::env::var("TELESH_RELAY").ok().and_then(|v| non_empty(&v)) {
        tracing::warn!(
            %url,
            "TELESH_RELAY is deprecated; use --relay-url/TELESH_RELAY_URL instead"
        );
        return Some(url);
    }
    if cfg.relay_serve.is_some() {
        tracing::warn!(
            "an embedded relay is running but its public URL is unknown; agents will use iroh's \
             default relays. Set --relay-url, the relay config file's `url`, or --public-host."
        );
    }
    None
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// `scheme://host[:port]`, omitting the default port.
fn http_url(scheme: &str, host: &str, port: u16, default_port: u16) -> String {
    if port == default_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}
