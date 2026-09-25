# 身份、账户与访问控制

状态：**P1–P3 implemented**；P4（密码/OIDC/实名、密钥轮换、
`wss://`）待完成。取代临时的 `agent_id` / `TELESH_SECRET` 模型。

P1 已实现：

- 为 `accounts`、`account_keys`、`sessions` 提供 SQLite 持久化（`sqlx`）
  （`crates/telesh-server/src/db.rs`）；DB 固定在 `<dataroot>/server.sqlite`。
- 密钥对挑战-应答登录 + 会话快速路径
  （`crates/telesh-server/src/control.rs`、`crates/telesh/src/auth.rs`）。
- 随机 9 位数字 `account_id`，逐位生成；在网络上和注册表中它是补零后的句柄
  （例如 `"000012345"`），在下游各处取代自申报的 `agent_id`。
- 新公钥首次登录即建账号并绑定(TOFU);注册策略以后再加开关。
- 会话令牌：不透明，以 SHA-256 存储，固定 12h TTL；
  由 agent 缓存在 v2 状态文件中，并在重连时出示。
- 线协议：`Hello` 携带身份/会话，另有
  `AuthChallenge`/`AuthResponse`/`AuthOk`/`AuthError` 帧；`agent_id`/
  `auth_key` 已从网络上消失。定义见 `design-control.md`。

P2 已实现：

- P2P 目录以 `"<account>/<name>"` 为键（`state::P2pEntry {
  account, name, endpoint_id, .. }`）；服务端为每个 `P2PAnnounce`
  标注**已认证**的账户，绝不使用客户端提供的账户。
- `P2PLookup { account, name }`；`P2PAddrResp::Found` 返回 `account` 和
  `name`；令牌的作用域为 `account/name`。
- 访问者以 `ACCOUNT/NAME` 形式寻址服务（`telesh connect 000012345/web`）。
- 在数据面上，流前导的 service 字段为 `account/name`；
  若某服务的账户与 agent 自身账户不同，agent 会拒绝它。

P3 已实现：

- **无服务端密钥。** 数据面的访问令牌是不透明的 256 位值；
  服务端仅在 `tokens` 表中存储其 SHA-256 哈希（作用域为
  `account_id` + `name`，短 TTL）。
- peer 从 `P2PLookup` 获得令牌，并在流前导中出示它
  （kind 1，与 `account/name` 一起）。
- agent 无法本地验证它，因此它**向服务端询问**
  （`token_check` → `token_check_resp`，见 `design-control.md`）；服务端是权威。这
  取代了旧的对称 HMAC 前导，后者可被 agent 伪造。

尚未完成：密码/OIDC/实名、密钥轮换、`wss://`（P4）。

## 1. 目标

- 为每个 agent 赋予密码学身份（`EndpointId`），并在登录时加以证明。
- 为人们提供**易于记忆的句柄**：由服务端签发的 9 位 `account_id`。
- 支持自助注册，日后可扩展至密码 / OIDC / 实名（实名）。
- 按账户为服务名建立命名空间，使名称在全局范围内永不冲突。
- 通过 bearer 令牌为 web/dashboard 提供会话，并设有 12h 硬上限。
- 封堵当前漏洞：可伪造的 `agent_id`、未经证明的 `EndpointId`，以及
  agent 能够伪造服务令牌。

## 2. 层次

| 层 | 内容 | 示例 | 创建者 | 稳定 | 机密 |
|---|---|---|---|---|---|
| 密码学身份 | Ed25519 公钥 = `EndpointId` | `8ab8…f790` | agent | 是 | 公开 |
| 账户 | 服务端分配的 `account_id`（9 位数字）+ 认证方式 | `573920146` | 服务端 | 是 | 否（句柄） |
| 会话 | 不透明 bearer 令牌，由服务端存储，TTL ≤ 12h | 随机 | 服务端 | 否 | 是 |

`EndpointId` 证明*你是谁*；`account_id` 是人的句柄；会话
是用于某次连接的短时凭证。

## 3. 登录

### 3.1 Agent（默认：密钥对）

```
agent                                          control server
  │ Hello(ver, client=agent, method=keypair,
  │       pubkey=EndpointId, session?) ──────────────►│
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

- `nonce` 由服务端随机生成、一次性使用、TTL 很短；被签名的消息做了
  域分隔，并包含 `server_id` 以阻止跨服务端重放。
- 验证成功 == 客户端持有该 `EndpointId` 对应的私钥。
- 不会向 agent 返回任何密钥：访问令牌不透明，由服务端在线
  验证（§7），因此没有可离线验证的内容。

### 3.2 会话快速路径（web / dashboard / 重连）

已持有会话令牌的客户端在 `Hello` 中出示它（HTTP 场景下则是
`Authorization: Bearer`）。服务端对照 DB 验证它并
跳过挑战。这是浏览器使用的路径；浏览器永远不需要持有
签名密钥。

未来的 web 认证（`method=password|oidc|realname`）会用相应的检查取代挑战，
并且仍返回会话令牌——令牌是
通用货币，因此会话处理不变。

### 3.3 会话

- 令牌：256 位随机、不透明；DB 仅存储 `SHA-256(token)`。
- 固定过期：自签发时刻起 `min(configured, 12h)`。超过上限不滑动。
- 字段：`account_id`、`client_kind`（agent|web|dashboard）、`created_at`、
  `expires_at`、`revoked`。
- 重新登录会签发新会话；登出即撤销。
- agent 可以保留会话令牌并在重连时复用，或在令牌过期时
  重新执行挑战-应答（无状态）。

## 4. 账户（自助 / TOFU）

- 首次成功的密钥对登录会创建账户并绑定该密钥。
- TOFU 在这里是安全的：没有他人私钥就无法占用其 `EndpointId`，
  因此不存在身份抢注。
- schema 支持每个账户多个密钥（多设备 / 轮换）；添加密钥的登录
  流程稍后推出（§13）。

## 5. 短 id（9 位数字）

- 在 `100_000_000..=999_999_999` 中随机（无前导零；9 位数字）。
- 通过 DB 唯一列 + 冲突时重试插入来保证唯一性。
- 它是**句柄，不是凭证**：知道它不授予任何权限；登录仍
  通过密钥/密码。因此枚举只能揭示存在性，而非访问权。
- 按原样显示（可选分组为 `573 920 146`）。

## 6. 命名空间化的服务

- 目录键变为 `(account_id, name)`，而非全局 `name`。
  两个账户都可以暴露 `web`。
- **服务端从已认证的会话推导 `account_id`，绝不从
  客户端提供的字段获取。**
- 在账户内，名称由宣告它的 `EndpointId` 拥有（与当前一致），
  因此同一账户的第二个 agent 无法静默接管它。
- 访问者寻址变为 `telesh connect <account-id>/<name>`
  （例如 `573920146/web`）。
- 按名称设置服务访问策略：`account`（默认，仅同一账户）、
  `public` 或 `allowlist`（见 §13）。

## 7. 令牌

两者都是**不透明的**；服务端是唯一的验证方（无签名密钥）。

| 令牌 | 形式 | 存储位置 | 验证方 | TTL | 用途 |
|---|---|---|---|---|---|
| 会话 | 不透明，256 位 | DB（SHA-256） | 服务端（查询 DB） | 12h | 控制连接 / web / dashboard |
| 访问 | 不透明，256 位 | DB（SHA-256），作用域为 `account_id`+`name` | 服务端，由 agent 请求 | 5 min | 在数据面上为 peer 授权某个 `account/name` |

- 服务端在 `P2PLookup`（以及每次重新宣告）时铸造访问令牌，并在
  `P2PAddrResp::Found` 中返回它。
- peer 在流前导中出示它（kind 1）。agent 无法离线验证
  它，因此它通过控制连接发送 `token_check` 并等待
  `token_check_resp`（`design-control.md` §4）。
- 这就是**无服务端密钥**的原因：验证是在线的
  （内省），而非离线签名检查。代价是
  每个数据面连接（包括纯 P2P）都必须能连到服务端。

## 8. 存储（sqlx + SQLite）

- 服务端拥有一个 DB,固定是 `<dataroot>/server.sqlite`(配置字段 `dataroot`,默认 `~/.telesh`)。
- `sqlx` 使用 `runtime-tokio` + `sqlite` + `migrate`；连接池 = `SqlitePool`。
- schema 由 `crates/telesh-server/migrations/` 中的 **sqlx migrations** 拥有
  （启动时执行 `sqlx::migrate!("./migrations")`；记录在 `_sqlx_migrations` 中）。
  不在代码中创建表。要更改 schema，请添加新的 `NNNN_*.sql` 文件。
  初始迁移使用 `IF NOT EXISTS`，因此迁移前的数据库
  （来自旧的 inline schema）可原地升级且不丢失数据。
- 保持类型可移植（INTEGER / BLOB / TEXT），以便未来的 Pro 构建可用
  相同查询切换到 Postgres。

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

## 9. 协议

线协议——传输、编码、握手消息以及完整消息
表——定义于 `design-control.md`。本文档仅涵盖协议所承载的
身份、账户、会话和令牌**模型**。

## 10. 配置

服务端只从 TOML 配置文件读配置,命令行只有一个参数(配置文件路径)。相关的字段:

| 配置字段 | 含义 |
|---|---|
| `dataroot` | 数据根目录;数据库是 `<dataroot>/server.sqlite` |

现在没有任何共享密钥类的配置:控制面认证完全基于密钥对 + session。注册策略(是否允许
匿名建账号、邀请码等)以后再加。

## 11. 安全

- **目前传输为 `ws://`**：挑战-应答保护身份，但
  链路可被窃听/中间人攻击。`wss://` 是后续的独立步骤。
- Nonce：一次性、检查 TTL、用 `server_id` 做域分隔。
- 对登录和账户创建做速率限制；在密码多次失败后锁定
  （一旦存在密码认证）。
- 会话令牌不透明且以哈希存储；登出时撤销；12h 硬上限。
- 服务（访问）令牌不透明，由服务端在线验证；没有
  签名密钥，因此没有任何东西可离线验证。
- 密码（未来）使用 argon2 存储；实名仅存储验证
  结果，绝不存储敏感明文。

## 12. 从当前模型迁移

| 现状 | 变更后 |
|---|---|
| `agent_id`（自申报） | 移除；`account_id` 来自登录 |
| `ClientHello.auth_key` | 挑战-应答 / 会话令牌 |
| `name` 目录（全局） | `(account_id, name)` 命名空间化 |
| HMAC 服务令牌 | Ed25519 服务端签名令牌 |
| `--state-file` 32 字节 `secret_key` | 不变：agent 的 Ed25519 身份 |

旧状态文件仍然可用（密钥即身份）；agent 的首次 v2 登录
会创建/绑定其账户。

## 13. 待办事项

- 命名空间化服务的访问者授权：默认为 `account`；`public` /
  `allowlist` 授权如何创建（所有者 CLI？dashboard？）。
- 密钥轮换 / 多设备：向账户添加第二个 `EndpointId`
  （由现有密钥签名）并指定主登录密钥。
- 注册体验：匿名(TOFU) vs. 邀请码 vs. OIDC。
- `wss://` 与证书处理。
- 会话是否可以在 12h 上限内刷新（滑动），还是严格
  固定。
