//! Dashboard HTTP API + built frontend hosting.
//!
//! 与控制面共用 `control_addr` 那个监听。页面之外的路径都在 `/api` 下:
//!   GET /api/clients            online agents
//!   GET /api/tunnels            registered tunnels (http + tcp)
//!   GET /api/metrics            aggregated counters
//!   GET /api/metrics/prometheus Prometheus text format
//!   GET /                       built frontend from server-ui/dist (if present)

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use rust_embed::RustEmbed;
use serde::Serialize;

use crate::metrics;
use crate::registry::Registry;
use crate::state::registry as get_registry;

/// The built frontend (server-ui/dist, produced by `pnpm build`) is embedded
/// into the binary at compile time. Build the frontend *before* compiling the
/// server: `(cd server-ui && pnpm build) && cargo build`.
#[derive(RustEmbed)]
#[folder = "../../server-ui/dist/"]
struct FrontendAssets;

#[derive(Clone)]
pub struct DashboardState;

pub fn router() -> axum::Router {
    axum::Router::new()
        .route("/api/clients", get(api_clients))
        .route("/api/tunnels", get(api_tunnels))
        .route("/api/metrics", get(api_metrics))
        .route("/api/metrics/prometheus", get(prometheus))
        .route("/", get(index))
        .route("/{*path}", get(assets))
        .with_state(DashboardState)
}

async fn index() -> impl IntoResponse {
    // 入口页面不缓存,否则浏览器会一直用旧的 index.html(包含旧的资源/图标 URL)。
    match FrontendAssets::get("index.html") {
        Some(file) => (
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Html(String::from_utf8_lossy(&file.data).into_owned()),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "frontend not built — run `pnpm build` in server-ui/",
        )
            .into_response(),
    }
}

/// Serve built static assets (js/css/svg/...) embedded at compile time.
async fn assets(State(_): State<DashboardState>, path: UrlPath<String>) -> impl IntoResponse {
    match FrontendAssets::get(&path) {
        Some(file) => {
            let mime = mime_for(&path);
            // Vite 产出的资源带内容 hash,可长缓存;其余(icon.svg 等)重新校验。
            let cache = if path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            (
                [
                    (axum::http::header::CONTENT_TYPE, mime),
                    (axum::http::header::CACHE_CONTROL, cache),
                ],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

// ---- API shapes ----

#[derive(Serialize)]
struct ClientRow {
    agent_id: String,
    sub_domain: String,
    online_secs: u64,
}

#[derive(Serialize)]
struct TunnelRow {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    /// Public entry: subdomain or TCP port.
    remote: String,
    status: &'static str,
    in_bytes: u64,
    out_bytes: u64,
}

#[derive(Serialize)]
struct TunnelsResponse {
    tunnels: Vec<TunnelRow>,
}

async fn api_clients() -> impl IntoResponse {
    let reg: &Registry = get_registry();
    let rows: Vec<ClientRow> = reg
        .list()
        .into_iter()
        .map(|c| ClientRow {
            agent_id: c.agent_id.clone(),
            // 一个 agent 现在可以挂多条 HTTP 隧道;这里把它们并起来展示。
            sub_domain: reg.sub_domains_of(&c.agent_id).join(", "),
            online_secs: c.connected_at.elapsed().as_secs(),
        })
        .collect();
    axum::Json(rows)
}

async fn api_tunnels() -> impl IntoResponse {
    let mut tunnels: Vec<TunnelRow> = Vec::new();
    // Host-routed (http) tunnels: one per registered subdomain.
    for (sub_domain, _route) in get_registry().routes() {
        let (in_bytes, out_bytes) = traffic_for(&format!("http:{sub_domain}"));
        tunnels.push(TunnelRow {
            name: sub_domain.clone(),
            kind: "http".to_string(),
            remote: sub_domain,
            status: "online",
            in_bytes,
            out_bytes,
        });
    }
    // Raw-TCP tunnels.
    for t in crate::tcp::list_tunnels() {
        let key = format!("tcp:{}", t.port);
        let (in_bytes, out_bytes) = traffic_for(&key);
        tunnels.push(TunnelRow {
            name: key,
            kind: "tcp".to_string(),
            remote: t.port.to_string(),
            status: "online",
            in_bytes,
            out_bytes,
        });
    }
    axum::Json(TunnelsResponse { tunnels })
}

fn traffic_for(key: &str) -> (u64, u64) {
    match metrics::metrics().traffic.get(key) {
        Some(t) => (
            t.in_bytes.load(std::sync::atomic::Ordering::Relaxed),
            t.out_bytes.load(std::sync::atomic::Ordering::Relaxed),
        ),
        None => (0, 0),
    }
}

async fn api_metrics() -> impl IntoResponse {
    axum::Json(metrics::snapshot())
}

async fn prometheus() -> impl IntoResponse {
    metrics::render_prometheus()
}