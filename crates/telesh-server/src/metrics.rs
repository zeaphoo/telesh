//! Unified server statistics.
//!
//! Both the dashboard JSON API and the Prometheus `/metrics` endpoint read
//! from this single source of truth, so the two views can never diverge.
//! Counters are plain atomics / a DashMap — no external metrics library.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use dashmap::DashMap;
use serde::Serialize;

/// Per-tunnel byte counters (key e.g. `http:<sub>` or `tcp:<port>`).
#[derive(Default)]
pub struct TrafficCounter {
    pub in_bytes: AtomicU64,
    pub out_bytes: AtomicU64,
}

pub struct Metrics {
    /// Currently connected agents.
    pub agents_online: AtomicU64,
    /// Live public connections being pumped.
    pub active_connections: AtomicU64,
    /// Host-routed (http) tunnels.
    pub http_tunnels: AtomicU64,
    /// Raw-TCP tunnels.
    pub tcp_tunnels: AtomicU64,
    /// Cumulative HTTP requests seen at the ingress (best-effort).
    pub requests_total: AtomicU64,
    /// Bytes flowing through each tunnel.
    pub traffic: DashMap<String, TrafficCounter>,
}

static METRICS: OnceLock<Metrics> = OnceLock::new();

pub fn init() {
    let _ = METRICS.set(Metrics {
        agents_online: AtomicU64::new(0),
        active_connections: AtomicU64::new(0),
        http_tunnels: AtomicU64::new(0),
        tcp_tunnels: AtomicU64::new(0),
        requests_total: AtomicU64::new(0),
        traffic: DashMap::new(),
    });
}

pub fn metrics() -> &'static Metrics {
    METRICS.get().expect("metrics not initialized")
}

/// Record bytes in/out for a tunnel. `key` should be stable per tunnel,
/// e.g. `http:mysite` / `tcp:18008`.
pub fn add_traffic(key: &str, in_bytes: u64, out_bytes: u64) {
    let m = metrics();
    let entry = m.traffic.entry(key.to_string()).or_default();
    if in_bytes > 0 {
        entry.in_bytes.fetch_add(in_bytes, Ordering::Relaxed);
    }
    if out_bytes > 0 {
        entry.out_bytes.fetch_add(out_bytes, Ordering::Relaxed);
    }
}

/// Snapshot shape for the dashboard JSON API.
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub agents_online: u64,
    pub active_connections: u64,
    pub http_tunnels: u64,
    pub tcp_tunnels: u64,
    pub requests_total: u64,
    pub traffic_in_bytes: u64,
    pub traffic_out_bytes: u64,
}

pub fn snapshot() -> MetricsSnapshot {
    let m = metrics();
    let (mut tin, mut tout) = (0u64, 0u64);
    for t in m.traffic.iter() {
        tin += t.in_bytes.load(Ordering::Relaxed);
        tout += t.out_bytes.load(Ordering::Relaxed);
    }
    MetricsSnapshot {
        agents_online: m.agents_online.load(Ordering::Relaxed),
        active_connections: m.active_connections.load(Ordering::Relaxed),
        http_tunnels: m.http_tunnels.load(Ordering::Relaxed),
        tcp_tunnels: m.tcp_tunnels.load(Ordering::Relaxed),
        requests_total: m.requests_total.load(Ordering::Relaxed),
        traffic_in_bytes: tin,
        traffic_out_bytes: tout,
    }
}

/// Render the same data as Prometheus text format (v0.4 exposition).
pub fn render_prometheus() -> String {
    let m = metrics();
    let mut out = String::new();
    let mut gauge = |name: &str, help: &str, value: u64| {
        out.push_str(&format!(
            "# HELP telesh_{name} {help}\n# TYPE telesh_{name} gauge\ntelesh_{name} {value}\n"
        ));
    };
    gauge("agents_online", "Currently connected agents", m.agents_online.load(Ordering::Relaxed));
    gauge("active_connections", "Live public connections being pumped", m.active_connections.load(Ordering::Relaxed));
    gauge("http_tunnels", "Host-routed http tunnels", m.http_tunnels.load(Ordering::Relaxed));
    gauge("tcp_tunnels", "Raw-TCP tunnels", m.tcp_tunnels.load(Ordering::Relaxed));

    out.push_str("# HELP telesh_requests_total Cumulative HTTP requests seen at the ingress\n# TYPE telesh_requests_total counter\ntelesh_requests_total ");
    out.push_str(&m.requests_total.load(Ordering::Relaxed).to_string());
    out.push('\n');

    for t in m.traffic.iter() {
        let name = t.key();
        let (tin, tout) = (
            t.in_bytes.load(Ordering::Relaxed),
            t.out_bytes.load(Ordering::Relaxed),
        );
        out.push_str(&format!(
            "# HELP telesh_traffic_in_bytes Bytes received from public\n# TYPE telesh_traffic_in_bytes counter\ntelesh_traffic_in_bytes{{tunnel=\"{name}\"}} {tin}\n"
        ));
        out.push_str(&format!(
            "# HELP telesh_traffic_out_bytes Bytes sent to public\n# TYPE telesh_traffic_out_bytes counter\ntelesh_traffic_out_bytes{{tunnel=\"{name}\"}} {tout}\n"
        ));
    }
    out
}

// ---- helpers used by call sites ----
pub fn inc_agents() {
    metrics().agents_online.fetch_add(1, Ordering::Relaxed);
}
pub fn dec_agents() {
    metrics().agents_online.fetch_sub(1, Ordering::Relaxed);
}
pub fn inc_conns() {
    metrics().active_connections.fetch_add(1, Ordering::Relaxed);
}
pub fn dec_conns() {
    metrics().active_connections.fetch_sub(1, Ordering::Relaxed);
}
pub fn inc_requests() {
    metrics().requests_total.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_http_tunnels() {
    metrics().http_tunnels.fetch_add(1, Ordering::Relaxed);
}
pub fn dec_http_tunnels() {
    metrics().http_tunnels.fetch_sub(1, Ordering::Relaxed);
}
pub fn inc_tcp_tunnels() {
    metrics().tcp_tunnels.fetch_add(1, Ordering::Relaxed);
}
pub fn dec_tcp_tunnels() {
    metrics().tcp_tunnels.fetch_sub(1, Ordering::Relaxed);
}