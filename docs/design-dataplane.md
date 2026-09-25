# 数据面:agent 主动连接 + yamux 分流

状态:**已实现**。取代 `design-p2p.md` §6 描述的 WebSocket 数据通道
(`open_stream_relay`、`Frame::Stream*`、`stream_id`)。

**范围**:本文只定义数据面。控制面的传输、编码与消息表在 `design-control.md`;
本文 §9.3 只说明两者的分工。

## 1. 目标与非目标

现状:控制 WebSocket 同时承载了被转发的公网连接。`ingress.rs::open_stream_relay`
把 `StreamPacket::Data{stream_id, data}` 经 `AgentConn::tx` 推出去,而 `tx` 就是
控制 WebSocket 的 sink。于是两类需求完全相反的业务共用一条有序字节管道:

- 控制消息:量小、频率低,需要 schema 演进;
- 转发的 payload:任意字节、量大,需要原始吞吐。

目标:

- 转发的 payload 不再和控制消息共用连接。
- 由 agent 主动发起数据连接。服务器无法主动连上一个在 NAT 后面的 agent,这与
  iroh 无关。
- 在这条 TCP 连接里用 [yamux](https://github.com/hashicorp/yamux/blob/master/spec.md)
  做分流(frp 的 work connection 用的就是同一套协议)。
- **TCP 连上之后只有 yamux 帧**。数据面的消息走一条专门的"信令流",分帧用
  `tokio-util` 的 `LengthDelimitedCodec`、payload 用 CBOR(§4);没有自造的
  `stream_id`、`Init`、`End`、`Refused`,也没有 mux 之前的握手前缀。
- **消息类型靠 CBOR 里的 `type` 演进**:未知类型解出来后忽略,所以以后加消息、新旧
  版本共存都不需要动分帧层。

本期非目标:

- TLS。先用裸 TCP(将来 TLS 也只是传输层,不引入应用层前缀)。
- 每个 agent 多条数据连接(先 1 条)。
- 控制面报文的增删(未定,见 §10)。

## 2. 拓扑与角色

```text
控制面   agent -> server   ws://<server>:<control_port>/tunnel   (不变)
                            登录、子域、TcpRegister、P2P 目录、保活、计量

数据面   agent -> server   tcp://<server>:<data_port>            (新增,独立端口)
                            TCP 连上即 yamux
                            stream 1            : 信令流(带帧,首条消息是鉴权,长期保持)
                            stream 2, 4, 6, ... : server 为每条公网连接开一条,纯字节
```

`open_stream()` 只有一条路:数据连接;agent 没连上来就拒绝该公网连接
(HTTP 返回 503,裸 TCP 直接关闭)。控制 WebSocket 不再承载任何 payload,
server 也不会主动去连 agent。

角色划分:

- **server 负责开公网连接的 stream**;公网连接到达的是边缘节点,让边缘开流最自然。
- **agent 只开一条 stream**,即 stream 1:信令流。首条消息是鉴权;这条流在数据连接
  存活期间保持打开,后续保活和将来的协商都走它。
- yamux `Mode`:**agent = `Mode::Client`**(奇数 stream id),**server =
  `Mode::Server`**(偶数 stream id)。这个 crate 里两种 Mode 都能开流
  (`Connection::poll_new_outbound` 没有 mode 限制,`next_stream_id` 只决定 id
  奇偶),所以"谁开哪些流"是约定而非限制。

数据端口从哪来:由服务端在握手时下发(`server_hello.success.data_addr`),agent 保持
零配置 —— 和 `relay_url` 同一个模式。服务端只知道自己 bind 的地址,所以下发地址的主机
是 `0.0.0.0` / `::` 时,agent 用自己连控制面时实际使用的主机替换(见
`design-control.md` §6)。

## 3. 数据监听的落点(已定:独立端口)

数据连接是承载 yamux 的裸 TCP,用 `tokio::net::TcpListener` 加自己的 accept 循环
即可,**不需要 axum**:axum 只在"复用 HTTP 端口"时才需要(要么实现
`axum::serve::Listener` 做字节嗅探,要么加一条 HTTP `Upgrade` 路由),而监听器存在
的唯一目的就是承载 yamux,HTTP 这一层帮不上忙。独立端口因此也是代码最少、
最容易验证的方案。

代价:多开一个端口(防火墙/安全组要放行)。若将来必须收敛到单端口,可换成"控制
端口 + HTTP `Upgrade` 路由"——数据协议本身不随落点改变。

## 4. 信令流(stream 1)与消息格式

TCP 连上后立即创建 yamux 会话,agent 打开第一条 stream(id 1)。这条流叫**信令
流**:上面的消息按下述方式分帧,第一条消息是鉴权;它在数据连接存活期间保持打开,
后续的保活与将来的协商都走它。

### 4.1 分帧:直接用 `tokio-util::LengthDelimitedCodec`

**不自己定义帧头**,分帧交给库,线格式就是它的默认形状:

```text
[u32 BE len][len 字节 payload]        payload = CBOR
```

```rust
// telesh_protocol::signal::SignalCodec 内部就是:
let codec = LengthDelimitedCodec::builder()
    .big_endian()                        // 默认
    .length_field_type::<u32>()          // 默认,4 字节
    .max_frame_length(MAX_SIGNAL_FRAME)  // 默认 8 MiB,这里收紧到 64 KiB
    .new_codec();
let framed = Framed::new(stream.compat(), SignalCodec::new());   // stream: yamux::Stream
```

- `SignalCodec` 的 `Item` 是 [`SignalMessage`]:已知消息或"未知类型(忽略)"。
- 库负责"数据未读全(`Ok(None)`)"与"数据有问题(`Err`)"的区分、长度字段位置、最大
  帧长检查(超限返回 `LengthDelimitedCodecError`),不用手写读循环。
- 这样**没有任何自定义帧头**:线上只有库的长度字段 + CBOR payload。这正是 gRPC
  Length-Prefixed-Message 的形状(少一个 compressed flag)。
- **消息类型放在 CBOR payload 里**(§4.2 / §4.3),没有 `type` / `flags` 字段。
- 帧格式只用在信令流上;`stream 2/4/6...` 上的公网 payload 完全不受影响。

**未知消息怎么处理**:type 在 payload 里,所以必须先解 CBOR 才知道类型。实现是
`SignalCodec` 先解成通用 CBOR 值取出 `type`,不在已知列表里就返回
`SignalMessage::Unknown` —— 调用方忽略该帧、**不断连**。这是"以后加消息不打断老
agent"的保证。

### 4.2 消息类型(payload 里的 `type` 字段)

`type` 用短字符串而不是数字:可读、可调试,不需要维护一张数字注册表,在 CBOR 里
字符串的开销也可以忽略。

| `type` | 方向 | 含义 |
|---|---|---|
| `"auth"` | agent → server | `AuthRequest` |
| `"auth_resp"` | server → agent | `AuthResponse` |
| `"ping"` / `"pong"` | 双向 | 信令流保活(见 §7) |
| `"limits"` | server → agent | 预留(未实现):动态调整 limits |
| `"goaway"` | 双向 | 预留(未实现):优雅下线(drain) |
| 其它 | — | 未知类型:`SignalMessage::Unknown`,**忽略**该帧,不断连 |

### 4.3 AuthRequest / AuthResponse

```text
AuthRequest (CBOR)                    AuthResponse (CBOR)
  type: "auth"                          type: "auth_resp"
  session: bytes                        status: "ok"
  agent_max_streams: u32                        | "bad_session"          会话无效/过期
                                                | "too_many_connections" 该 agent 已有数据连接
                                                | "internal"
```

`session` 是 CBOR 原生字节串(`serde_bytes`),不走 base64。

编码用 **CBOR**(已定,见 §9.1):字段按名字,加可选字段不需要升级;分帧由
§4.1 的 `SignalCodec`(`LengthDelimitedCodec` + CBOR)负责(见 §9.2)。

约束与保护:

- **为什么信令流放在 yamux 里,而不是 mux 之前**:这样 TCP 上除了 yamux 帧没有别的
  东西。端口扫描器发来的 `GET / HTTP/1.1` 会被 yamux 按版本字段判定为协议错误,
  立即 GoAway 断开,不需要写嗅探逻辑。
- 鉴权复用登录 session:server 用和控制面握手相同的校验(`session_account`),
  得到账号并把这条连接注册到该 agent 名下(带一个单调递增的 `run_id`,见 §8)。
- **只把第一条入站流当作信令流**;之后 agent 再开流一律关闭整条连接(agent 不需要
  再开流)。鉴权必须在 5s 内完成,否则关闭整条连接。
- 未鉴权连接的防护:server 只对已经鉴权的 agent 开流,所以未鉴权的数据连接不会承
  载任何公网流量。
- **帧错误即关闭整条数据连接**:长度超过 `max_frame_length`、CBOR 解不出来,都直接
  断开,不做"跳过重同步"的尝试(理由见 §9.2)。
- **未知 `type` 不关闭连接**:解成通用 CBOR 值后忽略该消息即可 —— 这是向前兼容的
  唯一要求,实现时必须照做,否则将来加消息会打断老 agent。
- server **必须**把开流数量钳制在 `agent_max_streams` 以内。yamux 对超过
  `max_num_streams` 的入站流是**终止整条连接**(其 `on_data` 里
  `Action::Terminate`),不是拒一条流——超了不会只失败一条,而是把数据连接和上面
  所有流一起打掉。所以鉴权消息里带上它,server 侧据此钳制自己开流的数量。
- 将来加 TLS 时,ALPN 在传输层协商,信令流的帧格式不变。

## 5. 流的生命周期

server 侧,每条公网 TCP 连接:

1. 取该 agent 的数据连接,`poll_new_outbound()` 得到 `yamux::Stream`(用
   `tokio_util::compat` 包成 tokio I/O);
2. 直接把公网 socket 的字节泵进流里,反向同理 —— **流里没有任何前缀**,收到的每
   个字节都是 payload;
3. EOF 时用 `poll_close()` 关闭。

stream 1 是信令流(§4),**永不承载 payload**;server 只为公网连接开偶数流。

- **流首有目标头** `[u16 BE len][target]`(§8):一条数据连接现在服务多个本地端口,
  服务端开流后先写目标,之后才是裸字节。编码与 iroh 前导的 `service` 字段**完全一致**
  (`telesh_protocol::stream::read_service` / `write_service` 一份实现);iroh 路径也把
  目标放进前导的 service 里,所以 agent 侧的目标表只有一份。
- **不需要 ack,但也拿不到显式的"拒绝"信号**。原始设想是让 agent 用 **RST** 表示
  本地 dial 失败,实现时发现:**yamux 把流被 reset 读成 `Ok(0)`(干净 EOF)**,和正常
  关闭在 API 上无法区分。所以采用的判据是"**agent 关闭前一个字节都没回**":对 HTTP
  隧道来说,合法响应至少有一个状态行,因此这种情况只可能是它连不上本地服务 →
  edge 回 `HTTP/1.1 502`。代价是这个判据不是协议信号而是可观测行为;好处是零 RTT、
  零额外字节。
- **半关闭必须显式**。`yamux::Stream` 没有 `poll_close()` 就 drop 会发 RST,可能
  丢数据(crate 的 `on_drop_stream` 明写了这一点)。所以:公网 socket EOF →
  对流 `poll_close()`(FIN);agent 对本地 socket 同样处理,用 `shutdown(Write)`
  而不是 reset。
- 真正的错误(鉴权失败、协议错误)直接关闭整条数据连接;单条流上的失败只是该流结束。

## 6. 改动清单(已实现)

| 文件 | 改动 |
|---|---|
| `crates/telesh-server/src/data.rs`(新增) | 数据端口监听、yamux driver、开流队列、信令流鉴权、数据连接注册 |
| `crates/telesh-server/src/config.rs` | 新增 `data_addr` 配置(默认 `0.0.0.0:9008`) |
| `crates/telesh-server/src/main.rs` | bind 数据端口、下发 `data_addr`、启动 acceptor |
| `crates/telesh-server/src/state.rs` | `DATA_ADDR` 下发;删除 `ACTIVE_STREAMS`、`NEXT_STREAM_ID` |
| `crates/telesh-server/src/registry.rs` | 数据连接表(`run_id` + 句柄);控制连接断开时一并关闭数据连接 |
| `crates/telesh-server/src/ingress.rs` | `open_stream()` = 数据面 → 503;删掉旧 WS relay 路径;yamux 泵用 select + 显式 FIN |
| `crates/telesh-server/src/control.rs` | 上报 `data_addr`;删掉 `Frame::Stream*` 的分发 |
| `crates/telesh-server/src/tcp.rs` | 不变;公网 TCP 监听仍然调 `open_stream()` |
| `crates/telesh/src/data.rs`(新增) | 数据连接客户端:dial、yamux、信令流(鉴权/保活)、accept payload 流、退避重连 |
| `crates/telesh/src/control.rs` | 登录后启动数据面客户端;删掉 `Frame::Stream*` 处理 |
| `crates/telesh/src/local.rs` | 改成按 yamux stream 工作,不再用 channel;本地拨号失败直接丢弃该流 |
| `crates/telesh/src/p2p.rs` | 只走 JSON text 帧 |
| `crates/telesh/src/inspect.rs` | 不再用服务端 `stream_id`,改用本地自增 id |
| `crates/telesh-protocol/src/signal.rs`(新增) | 信令流消息 + `SignalCodec`(`LengthDelimitedCodec` + CBOR) |
| `crates/telesh-protocol/src/message.rs` | 删除 `StreamPacket` / `Frame::Stream`;`ServerHello::Success` 增加 `data_addr` |

`yamux::Connection` 没有 Go 版那种 `Control` 句柄,所以 driver 任务自己持有连接,
通过 channel 暴露开流请求:

```rust
enum DataCmd {
    Open { reply: oneshot::Sender<Result<yamux::Stream, DataError>> },
}
```

driver 要同时照顾两件事:待处理的开流请求,以及连接本身的 I/O(包括 agent 开来的
第一条信令流)。两者都需要 `&mut Connection`,因此不能用两个分支的 `select!`
(会同时借用),要在同一个 `poll_fn` 里做:先轮询 `poll_new_outbound` 处理排队的
`Open`,返回 `Pending` 就保留该请求(yamux 会通过 ack-backlog 的 waker 唤醒),再
轮询 `poll_next_inbound` 推进 I/O 并接收信令流。做法是给连接套一层薄封装 + 一个
小状态机。

Cargo 依赖:`yamux = "0.14"`;`tokio-util` 需要 `compat`(为了 `TcpStream::compat()`)
和 `codec`(为了 `LengthDelimitedCodec`);CBOR 用 `ciborium`,二进制字段配
`serde_bytes`。

## 7. 限额与超时

`yamux::Config`(0.14)只暴露四个旋钮,相关的如下:

| 旋钮 | 默认值 | 建议 |
|---|---|---|
| `set_max_num_streams` | 512 | 调大(例如 4096)。它统计的是这条连接上的**全部**流(入站+出站),一个繁忙 agent 的并发公网连接很容易顶到默认值。 |
| `set_max_connection_receive_window` | 1 GiB | 必须 `>= 256 KiB * max_num_streams`;4096 条流正好需要 1 GiB,所以只调大 `max_num_streams` 而不调这个会失败。 |
| `set_split_send_size` | 16 KiB | 决定一条流最多占用多少线路长度后 yamux 才去穿插别的流——所有流共用一条 TCP,这个值直接关系到公平性。 |
| `set_read_after_close` | true | 保持默认。 |

**没有 per-stream 接收窗口的 setter**:每条流从 256 KiB 起步,yamux 根据连接 RTT
和该流带宽自动调优,上限受上面的连接窗口约束。(frp 的 Go fork 直接设 6 MiB 流窗
口,Rust crate 没有这个旋钮。)

其它限制:

- 本期每个 agent 一条数据连接。边缘按 `min(自身上限, 鉴权消息里的
  agent_max_streams)` 限制在途流数,超出就拒绝,不做无界排队。
- 首条鉴权消息超时:5s,超时即关闭整条数据连接。
- 保活走信令流上的 `Ping` / `Pong`(§4.2):yamux 空闲时**故意不发 ping**(它的
  `next_ping` 只在连接繁忙时运行),所以空闲数据连接会被 NAT/代理的空闲超时回收。
  有信令流之后,按固定间隔发 `Ping` 即可解决;TCP keepalive(`socket2` /
  `TcpKeepalive`)可以叠加,但不再必须。
- 控制连接结束时,agent 关闭数据连接;控制连接结束时,server 注销数据连接。
- 不要再引入无界缓冲:现在的 WebSocket relay 路径用
  `mpsc::unbounded_channel`,新路径直接把数据写进 yamux stream,让背压自然传导。

## 8. 失败与重连

- 数据连接断开:注册表清除该条目,在途的公网 socket 关闭(HTTP 502/504),agent
  以指数退避重连(1s → 30s),复用同一个 session。
- 重连顺序:每条数据连接带一个递增的 `run_id`;比当前值小的注册被拒绝,这样一次
  慢重连不会顶掉更新的连接。
- 控制连接断开:server 移除该 agent 及其 P2P 名字,并注销数据连接;agent 现有的
  `reconnect_loop!` 会把两者一起重建。
- 公网 HTTP/TCP 的 payload 只有一条通路:agent 主动连上来的数据连接。agent 没连上
  来时直接回 503 / 关闭。iroh 只用于 peer 直连被发布为 P2P 的目标,服务端不参与其
  payload。

## 9. 编码与定界

### 9.1 编码(数据面用 CBOR)

数据面的信令流消息用 **CBOR**:字段按名字、未知字段默认忽略、枚举按名字(不再有
bincode 那种下标错位)、可选字段可加而不改协议;`Vec<u8>` / `[u8;32]` 配
`serde_bytes` 走原生字节串。

控制面的编码(JSON)在 `design-control.md` §2,不在本文。两边按**平面**分开,而不是
按传输分开:同一批消息只定义一次编码,不因为跑在不同传输上而有两套实现。

### 9.2 数据面消息的分帧(已定:直接用 `tokio-util::LengthDelimitedCodec`)

不自己定义帧头,直接用 `tokio-util::codec` 的 `LengthDelimitedCodec`(§4.1):线格式
就是它的默认形状 `[u32 BE len][payload]`,payload 用 CBOR。

- **分帧是库的事**:库负责"没读全(`Ok(None)`)vs 有问题(`Err`)"、长度字段解析、
  最大帧长检查(超限返回 `LengthDelimitedCodecError`),不用手写读循环。
- **不自造帧头**:早先拟过 `[len][type][flags]`,那等于把 HTTP/2 的帧头和 gRPC 的
  长度前缀拼一遍,属于重复发明。`type` 放进 CBOR payload 就够了。
- **失步由三条兜住**:① `max_frame_length` 上限;② 任何长度 / 编码错误**关闭整条
  数据连接**,不做"跳过重同步";③ 带帧的只有信令流一条,失步不可能污染 payload 流
  (纯字节)或控制面。
- **未知消息**:type 在 payload 里,所以"跳过未知消息"实际是"解出来后忽略",而不是
  "按长度跳过"。信令流消息很小,这点开销可以接受,换来的是没有自造帧头。实现时必
  须宽松解码(见 §4.1),否则新消息会打断老 agent。

评估过、没采用的标准方案:

| 方案 | 形状 | 不采用的原因 |
|---|---|---|
| [RFC 8742](https://www.rfc-editor.org/rfc/rfc8742.txt) CBOR Sequences | CBOR 数据项直接拼接,零额外字节 | 规范明说"任何一项不合法就无法可靠解码后续";异步 Rust 里连续解 CBOR 项需要增量解析器(`ciborium` 是同步 API,且不报告消耗字节数);想廉价跳过元素还得按规范 §4.3 用 byte string 再包一层 |
| [gRPC Length-Prefixed-Message](https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md) | `[u8 compressed][u32 BE len][message]` | 形状几乎相同,但多了一个我们不需要的 compressed flag;长度同样在最前、能落到同一个库,所以等价于本方案 |
| [RFC 7464](https://www.rfc-editor.org/rfc/rfc7464.txt) JSON Text Sequences | `RS + JSON + LF` | 面向文本 JSON;编码已定为 CBOR 后不需要 |
| WebSocket 承载 yamux | RFC 6455 帧 + yamux | 能过只放行 HTTP 的代理,但 masking + HTTP upgrade,且与 yamux 分帧重复 |
| HTTP/2 取代 yamux | RFC 9113 帧 + 流 | 帧和流都标准,但实现量最大 |

字节序、长度字段宽度、`max_frame_length` 一旦确定就写进协议,不要随意改。

不要用 `tokio-serde-cbor`,它依赖已弃用的 `serde_cbor`(有安全公告,官方推荐替换成
`ciborium`);`tokio-serde` 的适配层可以留,格式自己接。

### 9.3 控制面(不在本文)

控制面(WS/HTTP)的传输、编码、握手与消息表**全部在 `design-control.md`**;
本文只定义数据面。两边互相独立:控制面用 JSON,数据面信令流用 CBOR。

## 参考

- yamux 协议规范(帧头、flag、流控):
  <https://github.com/hashicorp/yamux/blob/master/spec.md>
- `yamux` crate(0.14):<https://docs.rs/yamux/latest/yamux/>
- frp 的分层方式(裸 TCP 上先做可选 TLS,然后**立刻** yamux,`Login` /
  `NewWorkConn` 都在第一条 yamux stream 里读):`.links/frp/server/service.go`
  的 `HandleListener`(约 690 行)与 `handleConnection`(443 行)。
