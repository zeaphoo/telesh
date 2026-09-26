//! Persisted agent state: the Ed25519 identity, the last assigned subdomain,
//! and a cached login session.
//!
//! File layout (`v2`):
//! ```text
//! v2
//! <secret_key hex, 64 chars>
//! <subdomain, may be empty>
//! <session token hex, may be empty>
//! <session expiry, unix seconds, may be empty>
//! <node name, may be empty>
//! ```
//! Legacy files (no `v2` header) stored `agent_id / subdomain / secret_key`;
//! they are upgraded on load and the obsolete `agent_id` is dropped (the key is
//! the identity now).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct AgentState {
    /// Ed25519 identity. Its public half is the `EndpointId` and the account key.
    pub secret_key: [u8; 32],
    /// Last assigned subdomain (http tunnels).
    pub subdomain: Option<String>,
    /// Cached login session token, if any.
    pub session: Option<Vec<u8>>,
    /// Session expiry (unix seconds); 0 when unknown.
    pub session_exp: u64,
    /// Friendly node name shown to peers; hand-editable in the state file.
    pub name: Option<String>,
}

impl AgentState {
    pub fn fresh() -> Self {
        Self {
            secret_key: rand::random(),
            subdomain: None,
            session: None,
            session_exp: 0,
            name: Some(random_name()),
        }
    }

    /// The cached session, only if it is still valid for a while.
    pub fn live_session(&self) -> Option<Vec<u8>> {
        let session = self.session.clone()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Refresh a minute before expiry.
        if self.session_exp > now + 60 {
            Some(session)
        } else {
            None
        }
    }
}

/// 默认数据目录:`~/.telesh`;没有 HOME 时用当前目录下的 `.telesh`。
pub fn default_dataroot() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(".telesh"),
        Err(_) => PathBuf::from(".telesh"),
    }
}

/// 身份/会话文件:`<dataroot>/state`。
pub fn state_path(dataroot: &Path) -> PathBuf {
    dataroot.join("state")
}

/// Load state, generating a fresh identity if the file is absent.
pub fn load(path: &Path) -> Result<AgentState> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = text.lines().collect();
        if lines.first().map(|l| l.trim()) == Some("v2") {
            if let Some(mut state) = parse_v2(&lines) {
                // 旧版本可能没写节点名(第 5 行为空);补一个 LocalSend 风格的默认名并落盘。
                if state.name.is_none() {
                    state.name = Some(random_name());
                    let _ = save(path, &state);
                }
                return Ok(state);
            }
        } else if let Some(state) = parse_legacy(&lines) {
            // Upgrade in place.
            let _ = save(path, &state);
            return Ok(state);
        }
        // corrupt: fall through and regenerate
    }
    let state = AgentState::fresh();
    let _ = save(path, &state);
    Ok(state)
}

fn parse_v2(lines: &[&str]) -> Option<AgentState> {
    let secret_key = from_hex32(lines.get(1)?.trim())?;
    let subdomain = lines
        .get(2)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let session = lines
        .get(3)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .and_then(from_hex_vec);
    let session_exp = lines
        .get(4)
        .map(|s| s.trim())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let name = lines
        .get(5)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(AgentState {
        secret_key,
        subdomain,
        session,
        session_exp,
        name,
    })
}

fn parse_legacy(lines: &[&str]) -> Option<AgentState> {
    // Old layout: agent_id / subdomain / secret_key.
    let secret_key = lines
        .get(2)
        .map(|s| s.trim())
        .and_then(from_hex32)
        .unwrap_or_else(rand::random);
    let subdomain = lines
        .get(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(AgentState {
        secret_key,
        subdomain,
        session: None,
        session_exp: 0,
        name: Some(random_name()),
    })
}

pub fn save(path: &Path, state: &AgentState) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut text = String::from("v2\n");
    text.push_str(&to_hex(&state.secret_key));
    text.push('\n');
    text.push_str(state.subdomain.as_deref().unwrap_or(""));
    text.push('\n');
    if let Some(session) = &state.session {
        text.push_str(&to_hex(session));
    }
    text.push('\n');
    if state.session_exp > 0 {
        text.push_str(&state.session_exp.to_string());
    }
    text.push('\n');
    text.push_str(state.name.as_deref().unwrap_or(""));
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("cannot persist state to {}", path.display()))
}

/// Friendly default node name (LocalSend-style: adjective + animal).
fn random_name() -> String {
    use rand::Rng;
    const ADJECTIVES: &[&str] = &[
        "Brave", "Calm", "Clever", "Happy", "Swift", "Bright", "Quiet", "Bold", "Gentle",
        "Lucky", "Cosmic", "Silver", "Golden", "Silent", "Wild", "Warm", "Cool", "Merry",
        "Noble", "Sunny",
    ];
    const ANIMALS: &[&str] = &[
        "Otter", "Panda", "Fox", "Falcon", "Dolphin", "Tiger", "Heron", "Lynx", "Koala",
        "Raven", "Bison", "Crane", "Moth", "Puma", "Robin", "Seal", "Whale", "Wolf", "Wren",
        "Yak",
    ];
    let mut rng = rand::thread_rng();
    format!(
        "{} {}",
        ADJECTIVES[rng.gen_range(0..ADJECTIVES.len())],
        ANIMALS[rng.gen_range(0..ANIMALS.len())]
    )
}

/// Update only the session fields, preserving identity and subdomain.
pub fn save_session(path: &Path, session: &[u8], expires_at: u64) -> Result<()> {
    let mut state = load(path)?;
    state.session = Some(session.to_vec());
    state.session_exp = expires_at;
    save(path, &state)
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn from_hex32(s: &str) -> Option<[u8; 32]> {
    let v = from_hex_vec(s)?;
    v.try_into().ok()
}

fn from_hex_vec(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        out.push(u8::from_str_radix(s.get(i..i + 2)?, 16).ok()?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_a_missing_node_name() {
        let dir = std::env::temp_dir().join(format!("telesh-state-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = state_path(&dir);
        // v2,sub域/session/过期都为空,节点名那一行也是空的。
        std::fs::write(&path, format!("v2\n{}\n\n\n0\n\n", "00".repeat(32))).unwrap();

        let state = load(&path).expect("loads");
        assert!(
            state.name.as_deref().is_some_and(|n| !n.trim().is_empty()),
            "a handle name should be generated"
        );

        // 已落盘:再读一次拿到同一个名字,而不是重新生成。
        let again = load(&path).expect("reloads");
        assert_eq!(again.name, state.name);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
