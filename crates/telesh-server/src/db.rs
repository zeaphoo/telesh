//! Persistence for accounts, keys and sessions (SQLite via `sqlx`).
//!
//! Kept intentionally portable (INTEGER/BLOB/TEXT, plain SQL, runtime queries)
//! so a future Pro build can target PostgreSQL without rewriting queries.

use anyhow::{bail, Context, Result};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

/// Session lifetime: a fixed 12h ceiling (the design cap).
pub const SESSION_TTL_SECS: u64 = 12 * 3600;

/// Opaque data-plane access-token lifetime.
pub const TOKEN_TTL_SECS: u64 = 300;

/// 9-digit account handle, generated one digit at a time and combined.
fn random_short_id() -> u32 {
    let mut rng = rand::thread_rng();
    let mut id = 0u32;
    for _ in 0..9 {
        id = id * 10 + rng.gen_range(0..10u32);
    }
    id
}

/// Zero-padded handle form used on the wire and in logs, e.g. `"000012345"`.
pub fn account_handle(account_id: u32) -> String {
    format!("{account_id:09}")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn token_hash(token: &[u8]) -> [u8; 32] {
    Sha256::digest(token).into()
}

/// Open (creating if needed) the database and apply the schema.
pub async fn open(path: impl AsRef<std::path::Path>) -> Result<SqlitePool> {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open database {}", path.display()))?;
    // Schema is owned by sqlx migrations (`crates/telesh-server/migrations`).
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("database migration failed")?;
    purge_expired(&pool).await?;
    Ok(pool)
}

async fn purge_expired(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(now() as i64)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM tokens WHERE expires_at < ?")
        .bind(now() as i64)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mint an opaque access token for `account_id`/`name` and store its hash.
pub async fn create_token(pool: &SqlitePool, account_id: u32, name: &str) -> Result<Vec<u8>> {
    let token: [u8; 32] = rand::random();
    let expires_at = now() + TOKEN_TTL_SECS;
    sqlx::query(
        "INSERT INTO tokens(token_hash, account_id, name, created_at, expires_at)
         VALUES(?, ?, ?, ?, ?)",
    )
    .bind(&token_hash(&token)[..])
    .bind(account_id as i64)
    .bind(name)
    .bind(now() as i64)
    .bind(expires_at as i64)
    .execute(pool)
    .await?;
    Ok(token.to_vec())
}

/// Expiry of `token` as a live grant for `account_id`/`name`, else `None`.
pub async fn check_token(
    pool: &SqlitePool,
    account_id: u32,
    name: &str,
    token: &[u8],
) -> Result<Option<u64>> {
    let row = sqlx::query(
        "SELECT expires_at FROM tokens WHERE token_hash = ? AND account_id = ? AND name = ?",
    )
    .bind(&token_hash(token)[..])
    .bind(account_id as i64)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .map(|r| r.get::<i64, _>("expires_at") as u64)
        .filter(|&expires| expires > now()))
}

/// The account bound to an endpoint key, if any.
pub async fn account_for_key(pool: &SqlitePool, key: &[u8; 32]) -> Result<Option<u32>> {
    let row = sqlx::query("SELECT account_id FROM account_keys WHERE endpoint_id = ?")
        .bind(&key[..])
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<i64, _>("account_id") as u32))
}

/// Find the account for a key, creating one (with a fresh 9-digit id) on first
/// login. Safe under races: the key column is the unique anchor.
pub async fn account_for_key_or_create(pool: &SqlitePool, key: &[u8; 32]) -> Result<u32> {
    if let Some(a) = account_for_key(pool, key).await? {
        sqlx::query("UPDATE accounts SET last_login = ? WHERE account_id = ?")
            .bind(now() as i64)
            .bind(a as i64)
            .execute(pool)
            .await?;
        return Ok(a);
    }

    for _ in 0..32 {
        let id = random_short_id();
        let mut tx = pool.begin().await?;

        let created = sqlx::query(
            "INSERT INTO accounts(account_id, status, created_at) VALUES(?, 'active', ?)
             ON CONFLICT(account_id) DO NOTHING",
        )
        .bind(id as i64)
        .bind(now() as i64)
        .execute(&mut *tx)
        .await?;
        if created.rows_affected() == 0 {
            continue; // id collision, try another
        }

        let linked = sqlx::query(
            "INSERT INTO account_keys(endpoint_id, account_id, created_at) VALUES(?, ?, ?)
             ON CONFLICT(endpoint_id) DO NOTHING",
        )
        .bind(&key[..])
        .bind(id as i64)
        .bind(now() as i64)
        .execute(&mut *tx)
        .await?;
        if linked.rows_affected() == 0 {
            // Another request bound this key concurrently; drop ours and use theirs.
            tx.rollback().await?;
            return match account_for_key(pool, key).await? {
                Some(a) => Ok(a),
                None => bail!("account key race without a winner"),
            };
        }

        tx.commit().await?;
        return Ok(id);
    }
    bail!("could not allocate a unique account id")
}

/// Store/replace an account's friendly node name.
pub async fn set_display_name(pool: &SqlitePool, account_id: u32, name: &str) -> Result<()> {
    sqlx::query("UPDATE accounts SET display_name = ? WHERE account_id = ?")
        .bind(name)
        .bind(account_id as i64)
        .execute(pool)
        .await?;
    Ok(())
}

/// The account's friendly node name (empty if unset).
pub async fn display_name(pool: &SqlitePool, account_id: u32) -> Result<String> {
    let row = sqlx::query("SELECT display_name FROM accounts WHERE account_id = ?")
        .bind(account_id as i64)
        .fetch_optional(pool)
        .await?;
    Ok(row
        .and_then(|r| r.get::<Option<String>, _>("display_name"))
        .unwrap_or_default())
}

/// Mint a session token for `account_id`. Returns `(token, expires_at)`.
pub async fn create_session(
    pool: &SqlitePool,
    account_id: u32,
    client_kind: &str,
    ttl_secs: u64,
) -> Result<(Vec<u8>, u64)> {
    let token: [u8; 32] = rand::random();
    let expires_at = now() + ttl_secs;
    sqlx::query(
        "INSERT INTO sessions(token_hash, account_id, client_kind, created_at, expires_at)
         VALUES(?, ?, ?, ?, ?)",
    )
    .bind(&token_hash(&token)[..])
    .bind(account_id as i64)
    .bind(client_kind)
    .bind(now() as i64)
    .bind(expires_at as i64)
    .execute(pool)
    .await?;
    Ok((token.to_vec(), expires_at))
}

/// Resolve a session token to its account, if valid and unexpired.
pub async fn session_account(pool: &SqlitePool, token: &[u8]) -> Result<Option<u32>> {
    let row = sqlx::query(
        "SELECT account_id, expires_at FROM sessions WHERE token_hash = ? AND revoked = 0",
    )
    .bind(&token_hash(token)[..])
    .fetch_optional(pool)
    .await?;
    match row {
        Some(r) => {
            let expires_at = r.get::<i64, _>("expires_at") as u64;
            if expires_at <= now() {
                return Ok(None);
            }
            Ok(Some(r.get::<i64, _>("account_id") as u32))
        }
        None => Ok(None),
    }
}

/// Revoke a session (logout).
pub async fn revoke_session(pool: &SqlitePool, token: &[u8]) -> Result<()> {
    sqlx::query("UPDATE sessions SET revoked = 1 WHERE token_hash = ?")
        .bind(&token_hash(token)[..])
        .execute(pool)
        .await?;
    Ok(())
}
