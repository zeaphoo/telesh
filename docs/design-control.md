# 控制面协议(客户端 ↔ 服务端)

状态:已实现(控制面 JSON 化、多隧道发布、数据面能力下发均已落地)。

**这是控制协议的唯一出处。** 所有客户端模式共用它:`telesh http` / `telesh tcp` /
`telesh p2p` / `telesh connect`,以及数据面连接的注册。控制面**只承载小消息**,不承载
payload(payload 走 iroh 或数据面,见 `design-p2p.md`、`design-dataplane.md`)。

身份/账户/会话/令牌的**模型与存储**见 `design-identity.md`;本文只定义**线上协议**。

## 1. 传输与连接生命周期

| 项 | 内容 |
|---|---|
| 传输 | WebSocket `ws://<server>:<control_port>/api/tunnel` |
| 探活 | `GET /api/healthz`(纯 HTTP,不是消息) |
| 连接数 | 每个 agent 一条长连接;lookup-only peer 各自一条 |
| 保活 | server 每 30s 发 `ping`,90s 内没有收到任何帧则断开 |
| 首帧 | 必须在 10s 内到达 |
| 角色 | 由**首帧**决定:`hello` → agent;`lookup` → lookup-only peer |

角色差别:

- **agent**:注册子域 / TCP 端口,可收发隧道与控制消息,连接断开时服务端注销它的一切
  资源(子域、TCP 端口、该账号的 P2P 目录项、数据连接)。
- **lookup-only peer**(`telesh connect`):不注册任何东西,只发 `p2p_lookup` 并订阅目录
  更新;服务端不把它写进 registry。

HTTP 传输(可选,未定):同一套消息换成 JSON body,认证用
`Authorization: Bearer <session>`。

## 2. 编码(已定:JSON)

选 JSON 的理由(对比过 bincode / CBOR / JSON):

| 方案 | 控制面 | 数据面 | 评价 |
|---|---|---|---|
| 现状 | bincode(位置编码、枚举按下标、`allow_trailing_bytes`) | — | 加字段要改布局、插变体错位、多余字节被静默忽略 |
| A | HTTP=JSON、WS=CBOR | CBOR | 每种传输用自然编码;但同一批控制消息会有两套编码和两份测试 |
| **B(已定)** | JSON(不分传输) | CBOR | 按**平面**切分,没有任何消息被定义两次 |
| C | JSON | JSON | 全系统一套编码;但数据面的字节流没必要走 JSON |
| D | CBOR | CBOR | 一套编码;但 HTTP 侧调试需要 CBOR 工具 |

- 控制面的 payload 已经搬到数据面,剩下的都是小消息,字节字段只有公钥/session/token,
  base64 的 33% 开销可以忽略。
- WebSocket 自带消息边界,所以**不需要长度前缀**——现在那个 `[u32 BE len]` 是多余的。
- 数据面自己的信令流用 CBOR,见 `design-dataplane.md` §9.1。

具体规则:

- **WS:一条消息 = 一个 JSON text frame**。现有实现里的 `[u32 BE len]` 前缀和
  `MAX_FRAME_BODY` 那套二进制分帧删掉 —— WebSocket 自己就是分帧。
- **HTTP:一个请求/响应体就是一个 JSON 文档**,`Content-Type: application/json`。
- **信封**:serde adjacent tagging —— `{"type":"hello","data":{...}}`
  (`#[serde(tag = "type", content = "data", rename_all = "snake_case")]`)。`type` 是
  消息名(§4),`data` 是该消息的字段。
- **字段映射**:

| Rust | JSON |
|---|---|
| 固定 32 字节的密钥 / 标识(`public_key`、`EndpointId`) | **hex 字符串** |
| 不透明变长字节(session、token、签名) | **base64 字符串**(自写 serde module);不要用默认的数字数组 |
| `SocketAddr` | std 的 serde 在 human-readable 格式下就是字符串 `"1.2.3.4:8080"` |
| `EndpointAddr` | 显式映射为 `{"id":"<hex>","addrs":["relay:https://…","ip:1.2.3.4:80"]}`,**不依赖 iroh-base 自己的 serde 输出** |
| 枚举 | 变体名,不再是下标 |
| `u16` / `u32` / `u64` | JSON number(现有字段远小于 2^53,前端安全) |

线上实测样本(Python 客户端直连控制 WS):

```json
{"type":"server_hello","data":{"success":{"agent_id":"probe-1","relay_url":"http://127.0.0.1:18099","data_addr":"127.0.0.1:9008"}}}
```

兼容规则:

- JSON 默认忽略未知字段 → **加可选字段不用改协议**。
- 未知**消息类型**默认会报错 → 解码必须宽松:先解成 `serde_json::Value` 看 `type`,或
  给枚举留 catch-all。
- **未上线,不做版本协商**:不加 `PROTOCOL_VERSION` 之类的字段,也不用在握手时校验
  版本。`hello` / `lookup` 里不带协议版本号。

## 3. 握手与认证

### 3.1 agent:keypair(默认)

```
agent                                                    control server
  │ hello{ client_kind:"agent", method:"keypair",
  │        public_key, session?, enrollment?, display_name? } ──────────────►│
  │                                                                          │ session 有效 → auth_ok
  │ ◄────────────────────────── auth_challenge{ nonce, server_id, ts } ──────│
  │ sign("telesh-login-v1" ‖ server_id ‖ nonce ‖ ts.to_be_bytes())
  │ auth_response{ signature } ─────────────────────────────────────────────►│
  │                                       用 public_key 验签;首次登录 TOFU 建账号
  │                                       签发 session
  │ ◄────────────────────────── auth_ok{ account, session, expires_at, display_name }
  │ …… 之后的控制流量一律以 account(9 位 handle)为准 ……
```

- 签名载荷是**手工拼接**的,不是 JSON:`b"telesh-login-v1" ‖ server_id(32) ‖
  nonce(32) ‖ ts(u64 大端)`,Ed25519 签 64 字节。域分隔 + `server_id` 防跨服务器重放。
- `nonce` 服务端随机、单次使用、短 TTL。
- 首次成功登录建账号并绑定公钥(TOFU)。服务端配置了 `enrollment_secret` 时,首次登录必须
  同时带 `enrollment`,否则 `auth_error: enrollment_rejected`。

### 3.2 session 快路径

客户端已在本地缓存 session 时,直接在 `hello` 里带上(HTTP 用 `Authorization: Bearer`);
服务端查库校验,跳过 challenge。浏览器/dashboard 走这条路,不需要持有签名密钥。
服务端每次登录都会**签发新的 session**(轮换)。

### 3.3 lookup-only peer(`telesh connect`)

首帧用 `lookup` 而不是 `hello`:

```
lookup{ auth_key?, agent_id }        # agent_id 只是日志用的临时标识
```

- 鉴权独立于账号体系:服务端配置了 `--secret` / `--lookup-secret` 时必须匹配其中之一
  (`lookup_auth_ok`),两者都没配置则放行。
- 通过后服务端回 `server_hello.success`(`agent_id` 只是日志用的临时标识),随后它只
  回答 `p2p_lookup` 和保活。

### 3.4 server_hello

| 变体 | 含义 |
|---|---|
| `success` | 握手完成;字段见 §4。**lookup-only 连接也会收到** |
| `auth_failed` | 鉴权失败(仅 lookup 连接用) |
| `error(message)` | 其它错误 |

`server_hello` 是**能力下发**的位置:`success.relay_url` 告诉客户端用哪个 iroh relay
(见 §6)。

### 3.5 auth_error

`bad_signature` / `bad_session` / `session_expired` / `enrollment_required` /
`enrollment_rejected` / `unsupported_method` / `error(message)`。

## 4. 消息表

`type` 即 §2 信封里的消息名,与当前代码里的 `Frame` 变体一一对应。

| `type` | 方向 | 字段 | 用途 |
|---|---|---|---|
| `hello` | client → server(首帧) | `client_kind`, `method`, `public_key?`, `session?`, `enrollment?`, `display_name?` | agent 登录(**不再带子域**:隧道级信息在各次发布里) |
| `auth_challenge` | server → client | `nonce`, `server_id`, `ts` | 待签名挑战 |
| `auth_response` | client → server | `signature`(base64,64B) | 挑战签名 |
| `auth_ok` | server → client | `account`, `session`(base64), `expires_at`, `display_name` | 登录成功 |
| `auth_error` | server → client | 见 §3.5 | 登录失败 |
| `server_hello` | server → client | 见 §3.4 | 握手结果 + 能力下发 |
| `lookup` | peer → server(首帧) | `auth_key?`, `agent_id` | lookup-only 连接 |
| `ping` / `pong` | 双向 | 无 | 保活 |
| `tunnel_publish` | agent → server | `id`, `kind`(http/tcp), `target`, `name?`, `remote_port?` | 发布一条隧道;`id` 由 agent 生成,响应按它配对 |
| `tunnel_resp` | server → agent | `id` + `http{sub_domain,hostname}` / `tcp{port}` / `name_taken` / `invalid` / `error(msg)` | 发布结果 |
| `tunnel_close` | agent → server | `kind`(http/tcp/p2p), `key`(子域/公网端口/服务名) | 取消一条隧道 |
| `p2p_announce` | agent → server | `name`, `addr`(EndpointAddr), `local_port` | 注册 `account/name → EndpointAddr` |
| `p2p_announce_resp` | server → agent | `ok{name}` / `name_taken{name}` / `invalid_name{name}` | 每个名字的归属结果 |
| `p2p_lookup` | peer → server | `account`, `name` | 解析目录项 |
| `p2p_addr` | server → peer | `found{account,name,owner_name,addr,token?}` / `not_found` | 目录结果;也是订阅推送 |
| `traffic` | agent → server | `name`, `up`, `down` | 服务端看不到的 P2P 流量计数 |
| `tcp_mapped` | agent → server | `name`, `addr`, `local_port` | 路由器端口映射得到公网端点 |
| `token_check` | agent → server | `id`, `account`, `name`, `token` | 请服务端校验不透明访问令牌 |
| `token_check_resp` | server → agent | `id`, `ok`, `expires_at` | 令牌校验结果 |

`EndpointAddr` 来自 `iroh-base`;在 JSON 里按 §2 显式映射。

## 5. 会话与令牌

两者都是**不透明**的,服务端是唯一校验方(没有签发密钥)。

| 令牌 | 形式 | 存储 | 校验方 | TTL | 用途 |
|---|---|---|---|---|---|
| session | 不透明 256-bit | DB(SHA-256) | 服务端查库 | 12h 硬上限 | 控制连接 / web / dashboard |
| access | 不透明 256-bit,按 `account/name` 限定 | DB(SHA-256) | 服务端(由 agent 代问) | 约 5 分钟 | 授权某个 peer 访问某个服务 |

- 服务端在 `p2p_lookup`(以及每次重新 announce)时签发 access token,放在
  `p2p_addr.found.token` 里。
- peer 把它放在**数据面流前导**(kind 1,见 `design-p2p.md` §4.1)。agent 无法离线校验,
  于是发 `token_check` 给服务端并等 `token_check_resp`。所以数据面要求服务端可达——
  这是"不引入服务端签发密钥"的代价。
- session 只在控制面使用;数据面连接复用 session 做鉴权(见 `design-dataplane.md` §4.3)。

## 6. 服务端下发的客户端能力

原则:拓扑由服务端决定,客户端零配置。

| 能力 | 位置 | 状态 |
|---|---|---|
| `relay_url` | `server_hello.success.relay_url` | 已实现:客户端据此选 iroh relay;缺失表示用 iroh 默认(n0) |
| `data_addr` | `server_hello.success.data_addr` | 已实现:数据面 `<host>:<port>`。服务端只知道自己 bind 的地址,所以主机是 `0.0.0.0`/`::` 时由 agent 用控制连接实际使用的主机替换 |

## 7. 超时与限额

| 项 | 值 | 位置 |
|---|---|---|
| 首帧到达 | 10s | 服务端首帧读取 |
| 登录整体 | 客户端 15s 超时 | agent 侧 |
| 挑战应答读取 | 10s | 服务端 |
| 保活间隔 / 空闲断开 | 30s / 90s | 服务端 |
| 单条消息大小 | 需要上限(现在是 16 MiB 的 `MAX_FRAME_BODY`)| 换成 JSON 后按字节数限制 |
| 子域 / 服务名 | `[a-z0-9-]`,1..=63 | `validate_subdomain` |

错误处理原则:未知 `type` **忽略**,不断连;非法子域/名字用对应的 `*_resp` 或
`server_hello` 变体回绝,而不是静默丢弃。

## 8. 待定

- **是否支持 HTTP 传输**(以及认证头如何与 WS 路径共用)。
- **TLS**:控制面目前是 `ws://`,是公网部署的前置项,但不改消息集。
- **lookup 连接的鉴权是否并入统一体系**(现在走 `--secret` / `--lookup-secret`)。
