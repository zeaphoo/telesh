-- Accounts, keys, sessions and data-plane access tokens.
--
-- Managed by sqlx (`sqlx::migrate!`); do not create these tables in code.
-- `IF NOT EXISTS` keeps a pre-migration database (created by the old inline
-- schema) working on upgrade.

CREATE TABLE IF NOT EXISTS accounts (
    account_id   INTEGER PRIMARY KEY,
    display_name TEXT,
    status       TEXT    NOT NULL DEFAULT 'active',
    created_at   INTEGER NOT NULL,
    last_login   INTEGER
);

CREATE TABLE IF NOT EXISTS account_keys (
    endpoint_id  BLOB    PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(account_id),
    label        TEXT,
    created_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    token_hash   BLOB    PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(account_id),
    client_kind  TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    revoked      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS sessions_account ON sessions(account_id);

CREATE TABLE IF NOT EXISTS tokens (
    token_hash   BLOB    PRIMARY KEY,
    account_id   INTEGER NOT NULL,
    name         TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS tokens_expiry ON tokens(expires_at);
