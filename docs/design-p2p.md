# P2P、直连数据面与端口映射

状态：已实现。本文档描述在最初的仅中继隧道之上新增的 peer-to-peer 设计。

## 1. 目标

- **P2P 连接**：peer 通过 [iroh](https://www.iroh.computer/)（打洞的 QUIC）直接访问暴露的本地端口，并以加密中继作为自动回退。载荷永远不必经过 `telesh-server`。
- **端口镜像 / 映射**：通过 P2P 数据面暴露本地端口（`telesh p2p`），把远端服务镜像到本地端口（`telesh connect`），并可选地通过路由器（UPnP/PCP/NAT-PMP）打开一个真正的公网 TCP 端口。
- **无回归**：用于 HTTP 和原始 TCP 隧道的现有 WebSocket 中继路径继续可用，并仍是回退方案。

非目标：L3 覆盖网络/VPN，以及浏览器 WebRTC 客户端。控制面仍保留在现有的 WebSocket 上。

## 2. 架构

```text
  telesh connect <name>            telesh-server                 telesh p2p <port>
  (visitor)                        (control + directory +         (agent)
                                    optional relay/gateway)

  1) WS Visitor + P2PLookup ──────► directory lookup
  2) ◄───────────────────────────── P2PAddr{ EndpointAddr }
  3) iroh connect(EndpointAddr) ═══ hole-punched QUIC ═════════► Endpoint.accept()
       │                                                        accept_bi() → 127.0.0.1:<port>
       └── fallback → iroh relay (self-hosted or n0 default)

  HTTP/TCP ingress:
  public client → server ingress ─(iroh dial, fallback WS)────► agent → local service
```

- **控制面**（WebSocket）：认证、子域名分配、P2P 目录，以及直连面地址通告。
- **数据面**（iroh QUIC）：每条公网连接一个 iroh 双向流。仅当无法建立直连路径时才使用中继。

## 3. 身份

`crates/telesh/src/state.rs` 会持久化一个 32 字节的 iroh `SecretKey`，与 `agent_id` 并列存放。其公开部分就是 `EndpointId`，因此 agent 在重连和重启后保持同一身份。较旧的状态文件（没有密钥）会在加载时就地升级。

## 4. P2P 使用的控制面消息

帧格式、编码、握手以及**完整消息表**见 `design-control.md`。本节仅记录 P2P 特有的语义。

| 消息 | 语义 |
|---|---|
| `p2p_announce` | agent 注册 `account/name -> EndpointAddr`；account 来自已认证的会话，绝不来自客户端 |
| `p2p_announce_resp` | 按名称的归属结果；当*另一个* `EndpointId` 拥有该名称时为 `name_taken` |
| `p2p_lookup` | peer 解析 `account/name` |
| `p2p_addr` | 查询结果外加一个短期访问令牌；当某个名称被重新通告时，订阅者也会收到该推送 |
| `lookup` | 仅查询连接的第一帧；不注册任何内容 |
| `iroh_ready` | http/tcp agent 通告其直连面地址（变化时重新发送） |
| `traffic` | 服务端无法观测到的 P2P 流量计数 |
| `tcp_mapped` | 通过路由器端口映射获得的公网端点 |
| `token_check` / `token_check_resp` | agent 请求服务端校验一个不透明访问令牌 |

`EndpointAddr` 来自 `iroh-base`，并由 `telesh-protocol` 重新导出。

### 4.1 流前导

每个数据流的头几个字节携带一个凭证以及调用方想要的服务，随后是原始载荷：

```
kind 0 (shared secret): [0][u16 secret_len][secret][u16 service_len][service]
kind 1 (access token) : [1][u16 token_len][token][u16 service_len][service]
```

接受方自行校验 kind-0 secret；对于 kind-1 token，它会转发给服务端（`TokenCheck`）。然后它把 `service` 解析为本地端口。http/tcp agent 发送空的服务名，因为它们只暴露单个目标。接受方以 `STREAM_ACK_OK`/`STREAM_ACK_ERR` 作为应答。

## 4b. 多服务暴露

一个 `telesh p2p` 进程可以通过**单个** iroh endpoint（因而也是单个 `EndpointId` 和一个 UDP socket）暴露多个端口：

```bash
telesh p2p --expose web=8000 --expose db=5432
telesh connect web -l 8000
telesh connect db  -l 5432
```

每个名称都作为自己的目录条目被通告，并指向同一个 `EndpointAddr`；访问方在流前导中请求的名称决定本地目标。单个位置参数形式（`telesh p2p 8000 -s name`）仍然可用，并将名称默认为 endpoint id。

## 5. 角色

### 5.1 `telesh p2p <port>` / `--expose NAME=PORT`（暴露）

1. 加载已持久化的 `SecretKey`。
2. 构建一个 iroh `Endpoint`（ALPN `telesh/p2p/1`），使用服务端在 `ServerHello::Success` 中通告的 relay URL；端口映射（`TELESH_NO_PORTMAP`）是仅剩的本地环境变量开关。
3. 打开 WebSocket 控制连接，并为每个服务通告一个 `P2PAnnounce`（全部共享同一个 endpoint 地址）。服务端按名称回复 `P2PAnnounceResp`；已被*另一个* endpoint 拥有的名称会被拒绝（`NameTaken`），agent 会显式退出，而不是悄悄地以一个过期身份继续提供服务。
4. 接受 iroh 连接：每个入站双向流都会被认证（§7），请求的服务名被解析为本地端口，然后转发字节。
5. 保持控制连接存活（keepalive + 按服务的 `Traffic`）。

### 5.2 `telesh connect <name> -l <local-port>`（镜像）

1. 打开一个仅查询的控制连接（`Hello` 的客户端类型为 `Lookup`），发送 `P2PLookup`，并保持订阅：只要该名称被（重新）通告，服务端就会推送 `P2PAddr`。握手还会给出所通告的 relay URL，这正是镜像在握手之后才构建其 iroh endpoint 的原因。
2. 一个**监督者**任务始终保持恰好一条指向最新地址的活跃 iroh 连接。当连接断开*或*推送来一个*不同的*地址时，它会重连，因此以新地址重启的 agent 会被**无需重新查询**地接上。相同地址的重复通告会被忽略。
3. 监听本地端口。每条被接受的本地连接都会被挂接到一个活跃的 iroh 双向流上，并在链路恢复期间重试——但**仅在 agent 确认之前**（`STREAM_ACK_OK`），即在任何载荷被转发之前。确认之后我们就只转发，永不重试。等待窗口固定为 30s；明确拒绝（`STREAM_ACK_ERR`）会立即失败。
4. 目标是 `ACCOUNT/NAME`（例如 `telesh connect 000012345/web`）；account 是服务所有者的 9 位账号。
5. `run_connect` 仅在控制订阅本身结束时返回；调用方随后会重新订阅。

### 5.3 HTTP / 原始 TCP agent

`client::run_once` 还会额外构建一个 iroh endpoint，使用 `ServerHello::Success` 中通告的 relay，并通过 `IrohReady` 通告它，在地址变化时重新通告（例如当家庭 relay 连上之后——通过 `watch_addr()` 观测到）。这不会阻塞隧道建立。

服务端在注册表中按 `agent_id` 存储该地址。在入站时（`ingress::open_stream`），它首先尝试通过 iroh 拨号 agent；若出现任何失败（拨号错误、prelude 写入失败），则回退到原来的 WebSocket 多路复用中继路径（`open_stream_relay`）。

## 6. 服务端

- **目录**（`state::P2P_DIR`）：`"<account>/<name>" -> P2pEntry { account, name, endpoint_id, P2PAnnounce }`。名称**按账户划分命名空间**，因此两个账户可以都暴露 `web`。在账户内，一个名称**由其 `endpoint_id` 拥有**（一种密码学 peer 身份），并以原子方式认领：同一个 endpoint 可以重新认领/替换自己的条目（重连），不同的 endpoint 会被拒绝而不是覆盖所有者，被拒绝的认领**不会**推送给订阅者。当拥有该名称的 agent 的控制连接断开时，条目会被移除。
- **查询连接**不会被注册：`handle_lookup` 只应答 `P2PLookup` 和 keepalive。
- **网关**（`gateway.rs`）：服务端绑定自己的 iroh endpoint（临时身份，端口映射被禁用）并拨号 agent。用 `TELESH_NO_IROH=1` 禁用。
- **内嵌中继**（`--relay-serve <addr>`）：运行一个进程内的 iroh 中继，因此自托管部署无需外部中继。默认是纯 HTTP；TLS 和 ACME/Let's Encrypt 来自中继配置文件（`$TELESH_RELAY_CONFIG`，默认 `~/.telesh/relay.toml`），该文件会启用 HTTPS 和 QUIC 地址发现端口。
- **中继通告**：relay URL 会在握手期间通过 `ServerHello::Success` 推送给客户端，因此 agent 永远不需要为其配置。其解析顺序为：`--relay-url`/`TELESH_RELAY_URL`、中继配置文件的 `url`、由内嵌中继推导出的 URL（ACME 域名，或 `--public-host` 加上监听器的端口），然后是旧版的 `TELESH_RELAY` 环境变量。如果都不知道，客户端就停留在 iroh 的默认（n0）中继上。服务端自己的网关也使用同一个 URL，因此两端对中继的认知一致。

## 7. 安全

- iroh 连接是加密的，peer 的 `EndpointId` 由 QUIC/TLS 认证。仅凭 `EndpointId` 并不授予访问权限，因此 agent 会施加应用层检查。
- **流前导**：拨号方（一个 peer 或服务端边缘）在每个流的头几个字节发送一个带标签的凭证：
  - kind 0，共享密钥：`[0][u16 len][secret][u16 service_len][service]`
  - kind 1，访问令牌：`[1][u16 len][token][u16 service_len][service]`
  agent 在本地校验 kind-0 密钥，而 kind-1 令牌则通过询问服务端（`TokenCheck`）来校验。未配置密钥时，agent 接受 kind 0，与控制面一致。
- **流确认**：接受方在前导和本地拨号之后回复一个字节：`STREAM_ACK_OK`（载荷可以流动）或 `STREAM_ACK_ERR`（明确拒绝）。这是拨号方载荷前重试的提交点。
- **不透明访问令牌，在线校验**：peer 从 `P2PLookup` 获得一个短期令牌，并在流前导中出示它（kind 1）。agent 无法验证它，因此它会询问服务端（`TokenCheck` → `TokenCheckResp`）；服务端只存储令牌哈希，作用域为 `account_id` + `name`。**数据面上不需要服务端密钥，也不需要共享密钥。** 代价是：服务端必须可达才能校验——令牌是一份授权，而不是离线能力。单独的 `--lookup-secret`（或主密钥）用于把关谁可以首先进行查询。
- 前导是应用层授权，而不是传输加密；它与控制面的 `auth_key` 交换不同。

## 8. 端口映射

两件不同的事：

1. **iroh portmapper**（默认开启）：通过 UPnP/PCP/NAT-PMP 映射 endpoint 的 UDP 端口，以改善直连性。`TELESH_NO_PORTMAP=1` 会禁用它（避免 SSDP 探测 / 防火墙提示）。
2. **`telesh p2p --map-tcp`**：请求本地网关映射所暴露的 TCP 端口，使服务*无需* iroh 或服务端即可被公网访问。得到的 `IP:port` 会被记录日志，并通过 `TcpMapped` 上报。在 CGNAT、UPnP 被禁用等情况下会失败，此时隧道继续走 iroh。

`portmapper` 被 (2) 直接使用；iroh 在内部将其用于 (1)。注意 iroh 内置的 portmapper 只映射它自己的 UDP 端口，而不映射任意 TCP 端口。

## 9. 指标

服务端无法观测纯 P2P 载荷，因此由 agent 上报：

- 每条 P2P 流都在 agent 中计数（`P2P_BYTES_SENT` / `P2P_BYTES_RECV`）。
- `run_expose` 每 10s 发送一次带增量值的 `Traffic` 报告。
- 服务端把它们记录为流量计数器中的 `p2p:<name>`，可在仪表盘和 `/metrics` 中查看。
- 对于 HTTP/TCP iroh 网关路径，由服务端自己转发字节，并按通常的 `http:<sub>` / `tcp:<port>` 键计数。

## 10. 配置

Agent（全局标志）：

| 标志 | 环境变量 | 含义 |
|---|---|---|
| `--state-file` | `TELESH_STATE` | 身份文件（iroh 密钥 + 缓存的会话） |
| `--enroll <s>` | `TELESH_ENROLLMENT_SECRET` | 首次注册用的注册密钥 |
| `--no-reconnect` | | 退出而不是重连 |
| `--dashboard-addr` | `TELESH_INSPECTOR_ADDR` | 本地 inspector 地址 |

Agent 仅环境变量开关：`TELESH_NO_PORTMAP=1`（禁用 UDP 网关探测）。iroh 中继**不是** agent 的设置：由服务端在登录后通告。

`telesh p2p`：`<PORT>` 或 `--expose NAME=PORT`（可重复）、`-s/--name`、`--map-tcp`。
`telesh connect`：`<ACCOUNT/NAME>`、`-l/--local-port`。

服务端：

| 标志 | 环境变量 | 含义 |
|---|---|---|
| `--secret <s>` | `TELESH_SECRET` | 主密钥（令牌签名 + 查询认证） |
| `--lookup-secret <s>` | `TELESH_LOOKUP_SECRET` | 用于查询的较低权限密钥 |
| `--db <path>` | `TELESH_DB` | 账户/会话 SQLite（默认 `~/.telesh/server.sqlite`） |
| `--enrollment-secret <s>` | `TELESH_ENROLLMENT_SECRET` | 把关首次注册 |
| `--relay-serve <addr>` | `TELESH_RELAY_SERVE` | 运行内嵌 iroh 中继 |
| `--relay-url <url>` | `TELESH_RELAY_URL` | 通告给 agent（并由网关拨号）的 relay URL |

服务端仅环境变量开关：`TELESH_NO_IROH=1`（禁用直连数据面）；旧版的 `TELESH_RELAY` 仍作为 `--relay-url` 的回退被接受。

内嵌中继的 TLS/ACME 来自中继配置文件（`$TELESH_RELAY_CONFIG`，默认 `~/.telesh/relay.toml`）：`url`（通告给 agent 的公网 URL）、`tls_cert`、`tls_key`、`https_addr`、`quic_addr`、`acme`、`acme_email`、`acme_staging`、`acme_cache`。

## 11. 限制 / 未来工作

- 内嵌中继支持手动 PEM TLS 和 ACME/Let's Encrypt（在 HTTPS 监听器上使用 TLS-ALPN-01）；尚无其他证书模式。
- 访问令牌是不透明的且由服务端校验，因此每条数据面流都要求服务端可达。它们是授权，而不是每个 peer 的身份。
- 一条已接受的本地连接只会在流确认之前跨 P2P 重连进行重试；一旦载荷已经流动就不再重放（否则会在 peer 上重复数据）。当 30s 窗口到期时仍在排队的连接会被丢弃。
- 服务名归属只在 endpoint 连接期间有效。当它断开时名称会被释放，因此另一个持有主密钥的 peer 可以认领它。持久的 `name -> endpoint_id` 预留可以堵上这个缺口；用每个 agent 各自的凭证替代一个共享密钥才能妥善堵上它。
- 控制订阅是唯一会强制重新查询的东西：如果访问方的 WebSocket 断开，`run_connect` 会返回，调用方会重新订阅。仅地址变化从不会触发重新查询。
- 在短暂重连窗口期间到达的本地连接会为等待一条活跃连接最多等待 `CONNECT_TIMEOUT`，然后放弃。
- MSRV 为 1.91（iroh 1.2）。
