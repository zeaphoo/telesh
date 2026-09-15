//! Persisted agent identity: stable `agent_id` + last assigned subdomain,
//! so reconnects (and restarts) keep the same public URL.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rand::Rng;

#[derive(Debug, Clone)]
pub struct AgentState {
    pub agent_id: String,
    pub subdomain: Option<String>,
}

impl AgentState {
    pub fn fresh() -> Self {
        Self {
            agent_id: random_id(24),
            subdomain: None,
        }
    }
}

/// Default state file: `$TELESH_STATE` or `~/.telesh/state`, else cwd.
pub fn default_path() -> PathBuf {
    if let Ok(p) = std::env::var("TELESH_STATE") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".telesh").join("state");
    }
    PathBuf::from(".telesh-state")
}

/// Load state, generating a fresh agent_id if the file is absent.
pub fn load(path: &Path) -> Result<AgentState> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let mut lines = text.lines();
        let agent_id = lines.next().unwrap_or("").trim().to_string();
        if !agent_id.is_empty() {
            let subdomain = lines
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return Ok(AgentState { agent_id, subdomain });
        }
        // corrupt file: fall through and regenerate
    }
    let state = AgentState::fresh();
    let _ = save(path, &state);
    Ok(state)
}

pub fn save(path: &Path, state: &AgentState) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut text = state.agent_id.clone();
    if let Some(s) = &state.subdomain {
        text.push('\n');
        text.push_str(s);
    }
    std::fs::write(path, text).with_context(|| format!("cannot persist state to {}", path.display()))
}

fn random_id(len: usize) -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}