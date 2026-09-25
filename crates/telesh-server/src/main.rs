//! telesh-server binary entrypoint.

use std::net::SocketAddr;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use telesh_server::{config, control, dashboard, data, gateway, ingress, relay_config, state, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("telesh_server=info,telesh_server::ingress=debug,tower_http=info")),
        )
        .init();

    let mut cfg = Config::parse();

    // Relay configuration is loaded once: it decides both the embedded relay we
    // run (if any) and the URL announced to clients after login. A broken file
    // disables the embedded relay rather than silently serving plain HTTP.
    let relay_file = match relay_config::load() {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("failed to load relay config: {e:#}");
            if cfg.relay_serve.take().is_some() {
                tracing::error!("not starting the embedded relay without its config");
            }
            relay_config::RelayConfigFile::default()
        }
    };
    config::init(cfg.clone());

    state::init().await?;

    // Optional embedded iroh relay, for a fully self-hosted deployment. Kept
    // alive for the process lifetime; dropping it shuts the relay down.
    let _embedded_relay = match cfg.relay_serve {
        Some(addr) => start_embedded_relay(addr, &relay_file).await,
        None => None,
    };

    // The relay clients are told to use; also the relay the gateway dials
    // through, so both ends agree on the same relay URL.
    let relay_url = relay_config::advertised_url(&cfg, &relay_file);
    state::set_relay_url(relay_url.clone());

    // Direct data plane: bind the iroh endpoint the ingress uses to reach
    // agents. A failure here is non-fatal — ingress falls back to the relay.
    if env_flag("TELESH_NO_IROH") {
        tracing::info!("iroh gateway disabled (TELESH_NO_IROH); ingress will use the relay only");
    } else {
        let iroh_relay = relay_url.unwrap_or_default();
        match gateway::build_endpoint(&iroh_relay).await {
            Ok(endpoint) => state::set_server_endpoint(endpoint),
            Err(e) => {
                tracing::warn!("iroh gateway unavailable, ingress will use the relay only: {e:#}")
            }
        }
    }

    // Data plane: agents dial this and speak yamux over TCP. The bind address is
    // announced verbatim; an agent that sees an unspecified host (`0.0.0.0`/`::`)
    // substitutes the host it used for the control connection.
    let data_addr = cfg.data_addr;
    state::set_data_addr(Some(data_addr.to_string()));
    let data_listener = tokio::net::TcpListener::bind(data_addr).await?;

    let ctrl_addr = cfg.control_addr;
    let dash_addr = cfg.dashboard_addr;

    // HTTP control server (serves /tunnel WebSocket + /healthz).
    let app = control::router().into_make_service_with_connect_info::<SocketAddr>();
    let ctrl = async move {
        let listener = tokio::net::TcpListener::bind(ctrl_addr).await?;
        tracing::info!(%ctrl_addr, "control server listening (ws://.../tunnel)");
        axum::serve(listener, app).await?;
        Ok::<_, anyhow::Error>(())
    };

    // Dashboard (API + built frontend + prometheus).
    let dash = async move {
        let app = dashboard::router();
        let listener = tokio::net::TcpListener::bind(dash_addr).await?;
        tracing::info!(%dash_addr, "dashboard listening (http://.../)");
        axum::serve(listener, app).await?;
        Ok::<_, anyhow::Error>(())
    };

    tokio::try_join!(
        ctrl,
        ingress::run_ingress(),
        dash,
        data::run(data_listener)
    )?;
    Ok(())
}

/// Start the embedded iroh relay on `http_addr`.
///
/// Plain HTTP by default. With TLS/ACME in the relay config file it also
/// serves HTTPS and opens the QUIC address-discovery port, which improves hole
/// punching for clients behind the relay. The public URL announced to agents is
/// resolved separately by [`relay_config::advertised_url`].
async fn start_embedded_relay(
    http_addr: SocketAddr,
    file: &relay_config::RelayConfigFile,
) -> Option<iroh_relay::server::Server> {
    let mut server_config = iroh_relay::server::ServerConfig::default();
    let mut relay = iroh_relay::server::RelayConfig::new(http_addr);

    if !file.acme.is_empty() {
        let Some(https_addr) = file.https_addr else {
            tracing::error!("relay config: `https_addr` is required with `acme`");
            return None;
        };
        match build_acme_cert_config(file) {
            Ok(cert) => {
                let quic = file.quic_addr_or_default();
                relay.tls = Some(iroh_relay::server::TlsConfig::new(https_addr, cert));
                server_config.quic = Some(iroh_relay::server::QuicConfig::new(quic));
                tracing::info!(
                    domains = ?file.acme,
                    staging = file.acme_staging,
                    %https_addr,
                    %quic,
                    "embedded iroh relay: ACME (Let's Encrypt) + QUIC address discovery"
                );
            }
            Err(e) => {
                tracing::error!("failed to configure relay ACME: {e:#}");
                return None;
            }
        }
    } else if let (Some(cert), Some(key)) = (&file.tls_cert, &file.tls_key) {
        let Some(https_addr) = file.https_addr else {
            tracing::error!("relay config: `https_addr` is required with `tls_cert`/`tls_key`");
            return None;
        };
        match build_relay_tls(cert, key) {
            Ok(tls) => {
                let quic = file.quic_addr_or_default();
                relay.tls = Some(iroh_relay::server::TlsConfig::new(
                    https_addr,
                    iroh_relay::server::CertConfig::Manual {
                        server_config: tls,
                    },
                ));
                server_config.quic = Some(iroh_relay::server::QuicConfig::new(quic));
                tracing::info!(
                    %https_addr,
                    %quic,
                    "embedded iroh relay: HTTPS + QUIC address discovery"
                );
            }
            Err(e) => {
                tracing::error!("failed to load relay TLS material: {e:#}");
                return None;
            }
        }
    } else {
        tracing::info!(%http_addr, "embedded iroh relay listening over plain HTTP");
    }

    server_config.relay = Some(relay);
    match iroh_relay::server::Server::spawn(server_config).await {
        Ok(server) => Some(server),
        Err(e) => {
            tracing::error!("failed to start embedded iroh relay: {e}");
            None
        }
    }
}

/// Whether an env var is set to a truthy value ("1"/"true"/"yes").
fn env_flag(key: &str) -> bool {
    matches!(
        std::env::var(key).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Build the ACME/Let's Encrypt certificate config for the embedded relay.
fn build_acme_cert_config(
    file: &relay_config::RelayConfigFile,
) -> anyhow::Result<iroh_relay::server::CertConfig> {
    let server_config_builder = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| anyhow::anyhow!("tls protocol versions: {e}"))?
    .with_no_client_auth();

    let mut acme =
        iroh_relay::server::AcmeConfig::letsencrypt(!file.acme_staging).domains(file.acme.clone());
    if let Some(email) = &file.acme_email {
        acme = acme.contact(vec![format!("mailto:{email}")]);
    } else {
        tracing::warn!("relay config has no `acme_email`; some ACME servers reject such accounts");
    }
    if let Some(cache) = &file.acme_cache {
        acme = acme.cache_path(cache.clone());
    }

    Ok(iroh_relay::server::CertConfig::LetsEncrypt {
        acme_config: acme,
        server_config_builder,
    })
}

/// Build a rustls server config from a PEM certificate chain and key.
fn build_relay_tls(
    cert: &std::path::Path,
    key: &std::path::Path,
) -> anyhow::Result<rustls::ServerConfig> {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};

    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|e| anyhow::anyhow!("cannot open certificate {}: {e}", cert.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("cannot parse certificate {}: {e}", cert.display()))?;
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| anyhow::anyhow!("cannot read private key {}: {e}", key.display()))?;

    let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| anyhow::anyhow!("tls protocol versions: {e}"))?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| anyhow::anyhow!("tls certificate/key mismatch: {e}"))?;
    Ok(config)
}