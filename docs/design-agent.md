# 常驻 agent:端口发现与发布管理

状态:**已实现**(P1–P3)。实现要点与两处偏差:

- 流首目标头最终**复用了 iroh 前导的 `[u16 service_len][service]` 编码**
  (`design-dataplane.md` §8),两条传输共用一份读写实现。
- 重连竞争:每条控制连接带一个 `conn_id`,旧连接的清理只会删自己的路由/端口/目录项,
  不会误删重连后新连接的注册。
- 目标表按"当前已应用的条目"整体重建:同一个本地端口可以同时发布为 HTTP 和 TCP,
  逐条增删会互相误删。
- 镜像已经搬进 UI;`telesh connect` 子命令已删除。镜像那条 lookup 连接用本机 agent
  自己的身份登录服务端,不需要额外密钥。
- HTTP 判定**真的探测**:每个端口一个进程生命周期内探一次(发一次 `HEAD /`,700ms
  超时,8 并发),端口消失后清缓存以便服务重启后重探;没探过时回落到端口表启发式。

**问题**:现在一个 `telesh` 进程只服务一个隧道(`telesh http 8000 -s app` /
`telesh tcp 22` / `telesh p2p 22`),发布方式全在启动参数里。于是同一台机器开 5 个
端口就要 5 个进程、5 次登录、5 条控制连接 + 5 条数据连接,而且本地 UI 只是个被动的
请求日志页。

**目标**:一个常驻进程,唯一必需参数是 control server;在本机 UI 上看到本机监听的
端口,按需决定发布哪一个、以什么方式发布;随时开关,不用重启。

## 1. 目标形态

```
telesh <control-server>          # 唯一的必需参数
```

- 发布配置是 UI 上的**声明式状态**,进程负责 reconcile。
- 老的 `http` / `tcp` / `p2p` / `connect` 子命令**已全部删除**。

## 2. 进程与 CLI

| 参数 | 说明 |
|---|---|
| `<control-server>` | 唯一必需参数,控制服务器 `host:port` |
| `--state-file` | 身份 + 会话(现状保留) |
| `--dashboard-addr` | 本地 UI 地址,默认 `127.0.0.1:16345` |

进程内同时持有两种角色:

- **发布方**(agent):登录 + 数据连接,按配置注册 HTTP / TCP / P2P 隧道。
- **访问方**(visitor):为 UI 上配置的"镜像"建立 lookup-only 控制连接 + 本地监听。

两者都由同一份期望状态驱动。

## 3. 本机端口发现

- Linux:读 `/proc/net/tcp` 与 `/proc/net/tcp6` 取监听态(`0A`)条目,得到本地端口与
  socket inode;再扫 `/proc/<pid>/fd` 反查 inode → pid / 进程名。非 root 只能看到本
  用户的进程,这属于正常。
- 备选:`ss -ltnp`(依赖 iproute2)或 netlink `sock_diag`。
- 容器内 `/proc` 是命名空间的,看到的正好是容器内的端口,符合直觉。
- 每 2–3s 轮询做 diff;端口消失时把对应发布标为 `target_gone`,不静默删除配置。

## 4. HTTP 还是 TCP

端口号无法可靠判断,分三层:

| 层 | 做法 |
|---|---|
| 权威 | 用户在 UI 上选(HTTP / TCP / P2P) |
| 默认 | 端口表启发式:80/443/3000/5000/8000/8080/8443… 标为 HTTP 候选 |
| 探测(已实现) | 发一次 `HEAD / HTTP/1.0`,看首行是否 `HTTP/`,顺带取 `Server:` 头 |

探测会真的打到应用,所以做了三层保护:**每个端口只探一次**(进程生命周期内)、
700ms 超时 + 8 并发、端口消失后清缓存。`/api/ports` 用 `probed` 区分"检测到"和
"建议"(未探测时回落端口表)。

## 5. 发布模型与配置

配置是一组条目(desired state),每条声明"把哪个本地端口以什么方式发布",或者"把哪个
远端服务镜像到本地端口":

```json
[
  {"kind": "http",   "local_port": 8000, "sub_domain": "app"},
  {"kind": "tcp",    "local_port": 22,   "remote_port": null},
  {"kind": "p2p",    "local_port": 5432, "name": "db"},
  {"kind": "mirror", "local_port": 18000, "remote": "000012345/web"}
]
```


- `kind` 判别联合;`local_port` 在发布条目里是"要暴露的本地服务",在 mirror 条目里是
  "要绑定的本地监听端口"(字段名已说明含义,避免混淆)。
- 每条运行时有状态:`pending`(等待生效)/ `live`(已发布,带对外的 hostname / 端口 /
  服务名)/ `error(原因)` / `target_gone`(本地端口没了)。

## 6. 本地 UI 与 API

在现有 inspector(`/api/tunnels`、`/api/requests`、replay)上加一页:

```
GET  /api/ports     → 探测到的监听端口 [{port, pid, process, http_hint}]
GET  /api/publish   → 当前配置 + 每条运行状态
PUT  /api/publish   → 覆盖式提交(desired state),返回每条的新状态
```

前端:端口列表 + 一行一个选择(不发布 / HTTP / TCP / P2P / 镜像),名称输入框,状态徽标,
以及"镜像远端服务"的独立小节。

## 7. 控制面需要的改动(核心)

现在"一个 agent = 一个子域"(握手时绑定),必须变成"一个 agent = N 条隧道":

| 消息 | 变化 |
|---|---|
| `hello` | **不再带 `sub_domain`**,登录只建立会话 |
| `http_register { sub_domain?, target }` → `http_register_resp { ok{hostname} \| in_use \| invalid }` | 新增,每个 HTTP 隧道注册一次 |
| `tcp_register { requested_port?, target }` → `tcp_register_resp` | 已有,补 `target` |
| `tunnel_close { id }` | 新增,取消发布 |
| `p2p_announce { name, addr, local_port }` | 已有;`name` 充当 id |

`target` 是 agent 分配的**本地目标标识**(当前就是本地端口的十进制字符串,如
`"8000"`)。服务端只存不解释,开流时原样回传。

路由表从"agent 级"下移到"目标级":

```
by_host:      subdomain → (agent_id, target)
TCP_TUNNELS:  port      → (agent_id, target)
by_agent:     agent_id  → 控制连接 + 数据连接句柄(仅用于断连清理)
```

## 8. 数据面:流首目标头

### 8.1 是什么

一条数据连接里跑着 N 条 yamux stream,每条 stream 对应一条公网连接。一个进程现在服务
多个本地端口,**yamux 的 stream 本身不带任何元数据**,所以服务端必须在每条流的最前面
写上"这条流打哪个目标",之后才是原始字节:

```text
服务端 open_stream() 之后的第一段字节:

  [u16 BE target_len][target 字节]      ← 目标头,只出现在每条流的最前面
  然后是公网连接的原始字节(双向,没有任何包装)
```

例:target = `"8000"` 时,流首 6 个字节是 `00 04 38 30 30 30`。

agent 侧:读目标头 → 查本地目标表 → 拨号 → 两个方向裸泵。目标不存在或拨号失败 → 直接
丢弃该流(边缘看到"零字节回包"→ HTTP 502,与现在的拒绝判据一致,见
`design-dataplane.md` §5)。

### 8.2 一次 HTTP 请求的时序

```text
公网 client ──TCP──► edge
edge: 按 Host 查路由 → (agent_id, target="8000")
edge: 在这条数据连接上 open_stream()
edge ──► agent   [u16 len]["8000"]          ← 目标头
agent: 读目标头 → dial 127.0.0.1:8000
edge ──► agent   HTTP 请求原始字节
agent ──► edge   HTTP 响应原始字节
```

每条流只多一次极小的读取,不引入往返(不需要 ack)。

### 8.3 为什么不复用 iroh 的整段前导

iroh 路径的前导已经存在,而且 `service` 字段就是同一个概念:

```text
iroh:  [kind][u16 cred_len][cred][u16 service_len][service] → [u8 ack]
```

但数据面上 dialer 是**已经鉴权的 edge**(连接级鉴权在信令流完成),所以:

- 不需要每条流再带凭据;
- 不需要那个 ack(多一个 RTT;而且 yamux 把 reset 读成干净 EOF,ack 带来的"明确拒绝"
  收益有限)。

**建议**:抽出两个共享小函数 —— "读写长度前缀的目标字段"和"目标 → 本地地址解析" ——
两条传输都调用它们,前缀各自定义。这样目标编码只有一处实现,agent 侧目标表也只有一份。

### 8.4 为什么 target 用字符串而不是数字 id

- 自解释:`"8000"` 在日志和抓包里直接可读;
- 以后可以指向非端口目标(命名目标、unix socket),不用改协议;
- 每条流只写一次,开销可忽略。

## 9. P2P

基本不用改协议:一个进程暴露多个 P2P 服务、同一个 iroh endpoint、按 `service` 选目标
**已经实现**(`p2p.rs` 的 targets 表)。变化只是从 CLI 驱动改成 UI 驱动,以及在
`p2p_announce`/`tunnel_close` 之间做增删。

## 10. 镜像(connect)搬进 UI

- 每条 mirror 条目 = 一条 lookup-only 控制连接 + 一个本地监听端口;
- 目标仍然是 `ACCOUNT/NAME`;开关时动态 bind / unbind 本地端口;
- 状态同样进 `/api/publish`(`live` 时显示实际绑定的本地地址)。

## 11. 持久化与重连

- 配置写 `~/.telesh/publish.json`(与 state file 同目录),启动时读取并 reconcile。
- 探测到的端口不持久化。
- 控制连接断开 → 所有条目回落 `pending`;重连后由 reconcile 自动补齐(子域/服务名沿用
  配置里的值,不重新随机)。

## 12. 安全

- UI 能发布端口 = 能把内网服务挂到公网。UI 必须只 bind `127.0.0.1`(默认已是);考虑
  加一个本地 token(存放于 state 目录,前端首次访问带上)以挡 CSRF。
- 需要明确:任何能访问这个 UI 的本机用户都能发布端口。

## 13. 分阶段

| 阶段 | 内容 |
|---|---|
| P1 | 常驻进程 + 端口发现 + 只读 UI(能看到端口,还不能发布) |
| P2 | 控制面 `http_register` / `target` / `tunnel_close` + 数据面目标头 + UI 开关(HTTP / TCP) |
| P3 | P2P 开关 + mirror 搬进 UI + 持久化 + 重连补齐 |

## 14. 待定

- UI 是否允许非本机访问(目前只 bind `127.0.0.1`,倾向不做)。
