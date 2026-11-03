# 命令行界面(agent / server)

Web UI 负责看和改复杂状态;CLI 负责脚本、SSH 和服务器上的日常操作。两者的
数据源是同一个:agent 的 CLI 调本机 daemon 的 HTTP API,server 的 CLI 调
dashboard API;配置写入也走同一个 TOML 文件,写前做完整校验。

所有命令都支持 `--json`(agent)或 `--config`/`--json`(server),适合管道和
自动化。默认端口 / 配置路径可以按下面的说明覆盖。

## Agent:`telesh`

默认 dataroot 是 `~/.telesh`,本地面板是 `http://127.0.0.1:16345`。

```bash
telesh                              # 前台运行常驻 agent
telesh --dataroot /etc/telesh       # 指定数据目录
telesh open                         # 在浏览器打开面板
telesh status                       # 版本 / 账户 / 隧道状态
telesh ports                        # 本机监听端口,标出已发布方式
telesh logs                         # 最近请求
telesh logs -f                      # 持续输出新请求
telesh logs --id 42                 # 查看单条请求/响应报文
telesh account                      # 登录状态、账户、公网身份
```

写操作和隧道配置:

```bash
telesh http 8000 -s app             # 发布 HTTP 子域
telesh tcp 22 -r 10022              # 发布公网 TCP 端口
telesh p2p 5432 6379                # P2P 入站白名单(覆盖式)
telesh p2p --all                    # 允许所有本机端口
telesh p2p --none                   # 关闭 P2P 入站
telesh connect 000012345/web -l 18000  # 把远端服务镜像到本地
telesh publish ls                   # 查看发布条目
telesh publish rm http 8000         # 删除条目
```

这些子命令只对**正在运行的 agent** 生效:它们写的是 daemon 的 desired state,
不会另外起进程。agent 没运行时,提示里会给出启动方式;退出码 `125` 表示 agent
不可用。

## Server:`telesh-server`

默认依次查找配置文件:

1. `--config <FILE>` / `-c <FILE>` / `TELESH_SERVER_CONFIG`
2. `./server.toml`
3. `/etc/telesh/server.toml`
4. `~/.telesh/server.toml`

兼容旧用法:`telesh-server /etc/telesh/server.toml` 仍然是前台启动服务。

```bash
telesh-server                       # 前台运行(默认命令 serve)
telesh-server -c /etc/telesh/server.toml
telesh-server serve -c /etc/telesh/server.toml
telesh-server status                # 配置 + 进程 + 指标摘要
telesh-server clients               # 在线 agent
telesh-server tunnels               # 活动 HTTP/TCP 隧道
telesh-server metrics               # 指标
telesh-server metrics --prometheus  # Prometheus 文本
telesh-server open                  # 打开 dashboard
telesh-server open --print-only     # 只打印 dashboard URL
```

### 配置

不用手工编辑 TOML。`config set` 只改一个键,保留原有注释,写前用完整的
`Config` 校验;失败时不会落盘。改完需要重启 `telesh-server` 生效。

```bash
telesh-server config path
telesh-server config show           # 含默认值的有效配置
telesh-server config get public_host
telesh-server config set public_host tunnel.example.com
telesh-server config set control_addr 0.0.0.0:9009
telesh-server config set tcp_port_range 18000-18100
telesh-server config set relay.serve 0.0.0.0:8081
telesh-server config set relay.acme relay.example.com,relay2.example.com
telesh-server config unset relay.serve
telesh-server config keys
telesh-server config validate
```

可写键:

| KEY | 值 |
|---|---|
| `dataroot` | 数据根目录 |
| `control_addr` | 控制面 + dashboard 监听地址 |
| `ingress_addr` | 公网 HTTP/TCP 入口 |
| `data_addr` | agent 数据面入口 |
| `public_host` | 拼隧道 URL 用的公网后缀 |
| `tcp_port_range` | `START-END` |
| `relay.serve` | 内嵌 iroh 中继监听地址 |
| `relay.url` | 通告给 agent 的中继 URL |
| `relay.tls_cert` / `relay.tls_key` | 中继 TLS 证书 / 私钥 |
| `relay.https_addr` / `relay.quic_addr` | 中继 HTTPS / QUIC 监听 |
| `relay.acme` | 逗号分隔的 ACME 域名 |
| `relay.acme_email` | ACME 联系邮箱 |
| `relay.acme_staging` | `true` / `false` |
| `relay.acme_cache` | ACME 缓存目录 |

`relay.acme` 传空字符串表示清空列表;关闭内嵌中继用
`telesh-server config unset relay.serve`。

### 退出码

- `0`:成功;`status` 在服务未运行时也会打印配置和原因,方便排查。
- `1`:配置非法、服务未运行(需要读 API 的命令)或 API 返回错误。
- `125`:agent 侧约定,server 的运维命令不使用。

## 注意

- server 的配置是启动时读入的;CLI 改完会明确提示重启。监听地址、TLS、ACME
  这类改动无法热加载。
- `status` / `clients` / `tunnels` / `metrics` 只读 dashboard API,不会修改
  运行中的进程。
- 配置写操作使用临时文件 + `rename` 原子替换,并尽量保留原文件权限。
