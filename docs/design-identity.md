# Identity, accounts and access control

Status: **P1–P3 implemented**; P4 (password/OIDC/real-name, key rotation,
`wss://`) pending. Supersedes the ad-hoc `agent_id` / `TELESH_SECRET` model.

Implemented in P1:

- SQLite persistence (`sqlx`) for `accounts`, `account_keys`, `sessions`
  (`crates/telesh-server/src/db.rs`); default DB `~/.telesh/server.sqlite`.
- Keypair challenge-response login + session fast path
  (`crates/telesh-server/src/control.rs`, `crates/telesh/src/auth.rs`).
- Random 9-digit `account_id`, generated one digit at a time; on the wire and in
  the registry it is the zero-padded handle (e.g. `"000012345"`), which replaces
  the self-declared `agent_id` everywhere downstream.
- Optional `--enrollment-secret` gates first-time registration.
- Session tokens: opaque, stored as SHA-256, fixed 12h TTL;
  cached by the agent in the v2 state file and presented on reconnect.
- Protocol v2 (`PROTOCOL_VERSION = 2`): `Hello` carries identity/session, new
  `AuthChallenge`/`AuthResponse`/`AuthOk`/`AuthError` frames; `agent_id`/
  `auth_key` are gone from the wire.

P2 implemented:

- The P2P directory is keyed by `"<account>/<name>"` (`state::P2pEntry {
  account, name, endpoint_id, .. }`); the server qualifies every `P2PAnnounce`
  with the **authenticated** account, never a client-supplied one.
- `P2PLookup { account, name }`; `P2PAddrResp::Found` returns `account` and
  `name`; tokens are scoped to `account/name`.
- Visitors address services as `ACCOUNT/NAME` (`telesh connect 000012345/web`).
- On the data plane the stream preamble's service field is `account/name`; the
  agent rejects a service whose account differs from its own.

P3 implemented:

- **No server key.** Data-plane access tokens are opaque 256-bit values; the
  server stores only their SHA-256 hash in the `tokens` table (scoped to
  `account_id` + `name`, short TTL).
- A peer receives the token from `P2PLookup` and presents it on the stream
  preamble (kind 1, together with `account/name`).
- The agent cannot verify it locally, so it **asks the server**
  (`Frame::TokenCheck` → `TokenCheckResp`); the server is the authority. This
  replaces the old symmetric-HMAC prelude, which agents could forge.

Not yet done: password/OIDC/real-name, key rotation, `wss://` (P4).

## 1. Goals

- Give every agent a cryptographic identity (`EndpointId`) and prove it at login.
- Give people a **memorable handle**: a 9-digit `account_id` issued by the server.
- Support self-service signup, extensible to password / OIDC / real-name (实名) later.
- Namespace service names per account so names never collide globally.
- Sessions for web/dashboard via a bearer token, with a hard 12h ceiling.
- Close the current holes: spoofable `agent_id`, unproven `EndpointId`, and
  agents being able to forge service tokens.

## 2. Layers

| Layer | What | Example | Created by | Stable | Secret |
|---|---|---|---|---|---|
| Crypto identity | Ed25519 public key = `EndpointId` | `8ab8…f790` | agent | yes | public |
| Account | server-assigned `account_id` (9-digit) + auth methods | `573920146` | server | yes | no (handle) |
| Session | opaque bearer token, server-stored, TTL ≤ 12h | random | server | no | yes |

`EndpointId` proves *who you are*; `account_id` is the human handle; the session
is the short-lived credential for a connection.

## 3. Login

### 3.1 Agent (default: keypair)

```
agent                                          control server
  │ Hello(ver, client=agent, method=keypair,
  │       pubkey=EndpointId, session?,enrollment?) ──►│
  │                                                   │ session valid? → AuthOk
  │ ◄──────────────── AuthChallenge(nonce, server_id, ts)
  │ sign("telesh-login-v1" ‖ server_id ‖ nonce ‖ ts)
  │ AuthResponse(signature) ─────────────────────────►│
  │                                          verify with pubkey
  │                                          find-or-create account (TOFU)
  │                                          mint session token
  │ ◄──────────────── AuthOk(account_id, session_token, expires_at, server_pubkey)
  │ ……subsequent control traffic is keyed by account_id……
```

- `nonce` is server-random, single-use, short TTL; the signed message is
  domain-separated and includes `server_id` to stop cross-server replay.
- Verify success == the client holds the private key for that `EndpointId`.
- `server_pubkey` is returned so the agent can verify **service access tokens**
  (§7) offline.

### 3.2 Session fast path (web / dashboard / reconnects)

Clients that already hold a session token present it in `Hello` (or as
`Authorization: Bearer` for HTTP). The server validates it against the DB and
skips the challenge. This is the path browsers use; browsers never need to hold
a signing key.

Future web auth (`method=password|oidc|realname`) replaces the challenge with
the corresponding check and still returns a session token — the token is the
common currency, so the session handling does not change.

### 3.3 Sessions

- Token: 256-bit random, opaque; the DB stores only `SHA-256(token)`.
- Fixed expiry: `min(configured, 12h)` from issue time. No sliding beyond the cap.
- Fields: `account_id`, `client_kind` (agent|web|dashboard), `created_at`,
  `expires_at`, `revoked`.
- Re-login issues a new session; logout revokes.
- Agents may keep a session token and reuse it on reconnect, or re-run
  challenge-response (stateless) when the token expired.

## 4. Accounts (self-service / TOFU)

- First successful keypair login creates the account and binds the key.
- TOFU is safe here: you cannot claim someone else's `EndpointId` without their
  private key, so there is no squatting on identities.
- Optional `--enrollment-secret`: when set, first login must also present it,
  closing open registration while keeping the flow self-service.
- Schema supports multiple keys per account (multi-device / rotation); the login
  flow for adding a key comes later (§13).

## 5. Short id (9 digits)

- Random in `100_000_000..=999_999_999` (no leading zero; 9 digits).
- Uniqueness via a unique DB column + insert-with-retry on conflict.
- It is a **handle, not a credential**: knowing it grants nothing; login is still
  by key/password. Enumeration therefore only reveals existence, not access.
- Display as-is (optionally grouped `573 920 146`).

## 6. Namespaced services

- The directory key becomes `(account_id, name)` instead of a global `name`.
  Two accounts can both expose `web`.
- **The server derives `account_id` from the authenticated session, never from a
  client-supplied field.**
- Within an account, a name is owned by the announcing `EndpointId` (as today),
  so a second agent of the same account cannot silently take it over.
- Visitor addressing becomes `telesh connect <account-id>/<name>`
  (e.g. `573920146/web`).
- Service access policy per name: `account` (default, same account only),
  `public`, or `allowlist` (see §13).

## 7. Tokens

Both are **opaque**; the server is the only verifier (no signing key).

| Token | Form | Stored | Verified by | TTL | Use |
|---|---|---|---|---|---|
| Session | opaque, 256-bit | DB (SHA-256) | server (DB lookup) | 12h | control conn / web / dashboard |
| Access | opaque, 256-bit | DB (SHA-256), scoped to `account_id`+`name` | server, asked by the agent | 5 min | authorizes a peer for one `account/name` on the data plane |

- The server mints an access token on `P2PLookup` (and on every re-announce) and
  returns it in `P2PAddrResp::Found`.
- The peer presents it on the stream preamble (kind 1). The agent cannot verify
  it offline, so it sends `Frame::TokenCheck { id, account, name, token }` over
  its control connection and waits for `TokenCheckResp { id, ok }`.
- This is why there is **no server key**: verification is on-line
  (introspection) rather than an offline signature check. The trade-off is that
  the server must be reachable for every data-plane connection, including pure
  P2P.

## 8. Storage (sqlx + SQLite)

- Server owns a DB (`--db <path>`, env `TELESH_DB`; default `~/.telesh/server.sqlite`).
- `sqlx` with `runtime-tokio` + `sqlite` + `migrate`; pool = `SqlitePool`.
- Schema is owned by **sqlx migrations** in `crates/telesh-server/migrations/`
  (`sqlx::migrate!("./migrations")` at startup; tracked in `_sqlx_migrations`).
  No table creation in code. Add a new `NNNN_*.sql` file to change the schema.
  The initial migration uses `IF NOT EXISTS` so a pre-migration database
  (from the old inline schema) upgrades in place without data loss.
- Keep types portable (INTEGER / BLOB / TEXT) so a future Pro build can switch to
  Postgres with the same queries.

```sql
CREATE TABLE accounts (
  account_id   INTEGER PRIMARY KEY,          -- 9-digit short id
  display_name TEXT,
  status       TEXT    NOT NULL DEFAULT 'active',
  created_at   INTEGER NOT NULL,
  last_login   INTEGER
);

CREATE TABLE account_keys (
  endpoint_id  BLOB    PRIMARY KEY,          -- 32-byte Ed25519 public key
  account_id   INTEGER NOT NULL REFERENCES accounts(account_id),
  label        TEXT,
  created_at   INTEGER NOT NULL
);

CREATE TABLE sessions (
  token_hash   BLOB    PRIMARY KEY,          -- SHA-256 of the opaque token
  account_id   INTEGER NOT NULL REFERENCES accounts(account_id),
  client_kind  TEXT    NOT NULL,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,
  revoked      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE services (
  account_id   INTEGER NOT NULL REFERENCES accounts(account_id),
  name         TEXT    NOT NULL,
  endpoint_id  BLOB,                         -- current owner (transport identity)
  access       TEXT    NOT NULL DEFAULT 'account',  -- account|public|allowlist
  updated_at   INTEGER NOT NULL,
  PRIMARY KEY (account_id, name)
);
```

## 9. Protocol changes (bump `PROTOCOL_VERSION` to 2)

- `Hello` (revised): `{ version, client_kind, method, public_key: Option<[u8;32]>,
  session: Option<Vec<u8>>, enrollment: Option<String>, sub_domain, … }`.
  `agent_id` and `auth_key` are removed.
- New: `AuthChallenge { nonce, server_id, ts }`,
  `AuthResponse { signature }`,
  `AuthOk { account_id, session_token, expires_at, server_pubkey }`,
  `AuthError { reason }`.
- `P2PAnnounce` / `P2PLookup` / `P2PAnnounceResp` now operate on
  `(account_id, name)`; `P2PLookup` carries the account id.
- ACK and stream-preamble kinds are unchanged.

## 10. Config

| Flag | Env | Meaning |
|---|---|---|
| `--db <path>` | `TELESH_DB` | server SQLite database |
| `--server-key <file>` | `TELESH_SERVER_KEY` | persistent Ed25519 key for service tokens |
| `--enrollment-secret <s>` | `TELESH_ENROLLMENT_SECRET` | optional; closes open registration |

`TELESH_SECRET` / `visitor_secret` are retired from the data plane; they survive
only, if at all, as the enrollment secret.

## 11. Security

- **Transport is `ws://` for now**: challenge-response protects identity, but the
  link is eavesdroppable/MITM-able. `wss://` is a later, independent step.
- Nonce: single-use, TTL-checked, domain-separated with `server_id`.
- Rate-limit login and account creation; lock after repeated password failures
  (once password auth exists).
- Session tokens are opaque and stored hashed; revoke on logout; 12h hard cap.
- Service tokens are Ed25519-signed so only the server can mint them; agents
  verify offline with `server_pubkey`.
- Passwords (future) stored with argon2; real-name stores only the verification
  result, never sensitive plaintext.

## 12. Migration from the current model

| Now | Becomes |
|---|---|
| `agent_id` (self-declared) | removed; `account_id` from login |
| `ClientHello.auth_key` | challenge-response / session token |
| `TELESH_SECRET` (shared everywhere) | optional enrollment secret |
| `name` directory (global) | `(account_id, name)` namespaced |
| HMAC service token | Ed25519 server-signed token |
| `--state-file` 32-byte `secret_key` | unchanged: the agent's Ed25519 identity |

Old state files keep working (the key is the identity); an agent's first v2 login
creates/binds its account.

## 13. Open items

- Visitor authorization for a namespaced service: default `account`; how are
  `public` / `allowlist` grants created (owner CLI? dashboard?).
- Key rotation / multi-device: adding a second `EndpointId` to an account
  (signed by an existing key) and designating the primary login key.
- Enrollment ergonomics: invite codes vs. a single enrollment secret.
- `wss://` and certificate handling.
- Whether a session may be refreshed (sliding) within the 12h cap, or is strictly
  fixed.