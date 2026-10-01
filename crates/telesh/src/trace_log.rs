//! Persistent per-channel request logs.
//!
//! One NDJSON file per channel under `<dataroot>/traces/<channel>.jsonl`
//! (one [`RequestRecord`] per line). The in-memory rings in `inspect` hold the
//! newest [`PER_CHANNEL`] records; this module mirrors them to disk.
//!
//! Append-only, so the capture hot path never blocks on I/O: `inspect` enqueues
//! records and [`run`] drains the queue. Once a file passes [`COMPACT_AT`] lines
//! it is compacted back to the newest [`PER_CHANNEL`] (write tmp + rename, so a
//! crash never leaves a half-written file). A torn last line from a crash is
//! simply skipped when loading.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::inspect::RequestRecord;

/// How many records to keep per channel.
pub const PER_CHANNEL: usize = 1000;
/// Compact a channel file once it grows past this many lines.
const COMPACT_AT: usize = PER_CHANNEL * 2;

/// channel -> newest-first ring, as stored in memory.
pub type Rings = HashMap<String, VecDeque<RequestRecord>>;

/// channel -> `<dir>/<channel with ':' -> '_'>.jsonl`.
fn file_for(dir: &Path, channel: &str) -> PathBuf {
    dir.join(format!("{}.jsonl", channel.replace(':', "_")))
}

/// channel key from a file stem (`publish_3000` -> `publish:3000`).
fn channel_of(stem: &str) -> String {
    stem.replacen('_', ":", 1)
}

/// Load every channel log in `dir`, keeping the newest `PER_CHANNEL` records.
pub fn load(dir: &Path) -> Rings {
    let mut rings: Rings = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return rings;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Read backwards so we only parse the newest PER_CHANNEL valid lines.
        let mut ring: VecDeque<RequestRecord> = VecDeque::new();
        for line in text.lines().rev() {
            if ring.len() >= PER_CHANNEL {
                break;
            }
            if let Ok(rec) = serde_json::from_str::<RequestRecord>(line) {
                ring.push_front(rec);
            }
        }
        if !ring.is_empty() {
            rings.insert(channel_of(stem), ring);
        }
    }
    rings
}

/// Background writer: append each record, compacting when a file gets too big.
pub async fn run(dir: PathBuf, mut rx: mpsc::UnboundedReceiver<(String, RequestRecord)>) {
    let _ = tokio::fs::create_dir_all(&dir).await;

    // Per channel: open append handle, lines written this run, and the newest
    // `PER_CHANNEL` serialized lines (used to compact without re-reading).
    let mut handles: HashMap<String, tokio::fs::File> = HashMap::new();
    let mut lines: HashMap<String, usize> = HashMap::new();
    let mut buffers: HashMap<String, VecDeque<String>> = HashMap::new();

    while let Some((channel, record)) = rx.recv().await {
        let Ok(line) = serde_json::to_string(&record) else {
            continue;
        };
        let buf = buffers.entry(channel.clone()).or_default();
        buf.push_back(line.clone());
        while buf.len() > PER_CHANNEL {
            buf.pop_front();
        }

        if !handles.contains_key(&channel) {
            match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(file_for(&dir, &channel))
                .await
            {
                Ok(f) => {
                    handles.insert(channel.clone(), f);
                }
                Err(e) => {
                    tracing::warn!(%channel, "cannot open the trace log: {e}");
                    continue;
                }
            }
        }

        let handle = handles.get_mut(&channel).expect("just inserted");
        if handle.write_all(line.as_bytes()).await.is_err()
            || handle.write_all(b"\n").await.is_err()
        {
            // Drop the handle and retry on the next record.
            handles.remove(&channel);
            continue;
        }

        let n = lines.entry(channel.clone()).or_insert(0);
        *n += 1;
        if *n > COMPACT_AT {
            let snapshot: Vec<String> = buffers
                .get(&channel)
                .map(|b| b.iter().cloned().collect())
                .unwrap_or_default();
            if compact(&dir, &channel, &snapshot).await.is_ok() {
                *n = snapshot.len();
                // The rename replaced the inode; reopen on the next record.
                handles.remove(&channel);
            }
        }
    }
}

/// Rewrite a channel file with exactly `lines` (atomic).
async fn compact(dir: &Path, channel: &str, lines: &[String]) -> std::io::Result<()> {
    let path = file_for(dir, channel);
    let tmp = path.with_extension("jsonl.tmp");
    let mut content = String::new();
    for line in lines {
        content.push_str(line);
        content.push('\n');
    }
    tokio::fs::write(&tmp, content).await?;
    tokio::fs::rename(&tmp, &path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u64, channel: &str) -> RequestRecord {
        RequestRecord {
            id,
            channel: channel.to_string(),
            local: None,
            method: "GET".into(),
            path: "/".into(),
            status: Some(200),
            duration_ms: 1,
            req_bytes: 1,
            resp_bytes: 1,
            started_at: "00:00:00.000".into(),
            payload: String::new(),
            resp_payload: String::new(),
        }
    }

    #[tokio::test]
    async fn appends_and_loads_the_newest_per_channel() {
        let dir = std::env::temp_dir().join(format!("telesh-traces-{}", rand::random::<u64>()));
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(dir.clone(), rx));

        for id in 1..=(PER_CHANNEL as u64 + 5) {
            tx.send(("publish:3000".to_string(), rec(id, "publish:3000"))).unwrap();
        }
        tx.send(("p2p:8080".to_string(), rec(9999, "p2p:8080"))).unwrap();
        drop(tx);
        let _ = task.await;

        let rings = load(&dir);
        assert_eq!(rings["publish:3000"].len(), PER_CHANNEL);
        // newest kept: last id is PER_CHANNEL + 5
        assert_eq!(rings["publish:3000"].back().unwrap().id, PER_CHANNEL as u64 + 5);
        assert_eq!(rings["p2p:8080"].len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}
