# Packaging

本目录与 `scripts/package-*.sh` 负责把 telesh 的四个运行形态打成可分发产物：

| 形态 | 二进制 / 应用 | 前端资源 | 平台产物 |
|---|---|---|---|
| agent（headless） | `telesh` | `agent-ui/dist` 经 **rust-embed** 编进二进制 | Linux `deb` / `rpm`（systemd） |
| server（headless） | `telesh-server` | `server-ui/dist` 经 **rust-embed** 编进二进制 | Linux `deb` / `rpm`（systemd） |
| 独立命令行工具 | `telesh` / `telesh-server` | 各自 headless 二进制已嵌入前端 | Linux/macOS `tar.gz`、Windows `zip` |
| agent 桌面客户端 | `telesh-app` | 随包 sidecar `telesh-sidecar`（其内部已嵌入前端） | Linux `deb`/`AppImage`、Windows `NSIS`/`MSI`/便携 `zip`、macOS `dmg` |
| server 桌面客户端 | `telesh-server-app` | 随包 sidecar `telesh-server-sidecar`（其内部已嵌入前端） | 同上 |

**资源嵌入原则**：所有 web 资源（JS/CSS/字体/图标）都在编译期通过
`rust-embed` 进入 `telesh` / `telesh-server`，安装包里没有散落的 `dist/` 目录。
桌面客户端不重复打包一份前端，而是携带对应的 headless 二进制作为 Tauri
`externalBin`（Linux/Windows 落在主程序同目录，macOS 落在 `Contents/MacOS/`）。
sidecar 文件名特意用 `telesh-sidecar` / `telesh-server-sidecar`：Linux deb 会把
`externalBin` 装到 `/usr/bin/`，若与 headless 服务包的 `telesh` / `telesh-server`
同名，两个包会争抢同一个文件。

`crates/*/build.rs` 是最后一道闸门：**release 构建**发现 `*-ui/dist/index.html`
缺失会直接失败；debug 构建允许缺失（面板返回 404），方便干净检出跑测试。
只想要 headless CLI/服务、不需要面板的源码构建可以显式放行：
`TELESH_ALLOW_EMPTY_UI=1 cargo build --release`（正式打包不要设置它）。
打包脚本在编译前统一执行 `scripts/build-ui.sh`，并在编译后校验带 hash 的 JS
资源确实出现在二进制里。

## GitHub Release

发布由 **`v*` 标签**触发（`.github/workflows/release.yml`），一次 tag 产出并挂载：

| 平台 | 产物 | 构建脚本 |
|---|---|---|
| 命令行工具（全平台） | `telesh_<ver>_<os>_<arch>.tar.gz` / `.zip`；server 同名规则 | `scripts/package-cli.sh` |
| Linux 服务（agent） | `telesh_<ver>-1_linux_<arch>.deb`、`telesh-<ver>-1.linux.<arch>.rpm` | `scripts/package-linux.sh` |
| Linux 服务（server） | `telesh-server_<ver>-1_linux_<arch>.deb`、`telesh-server-<ver>-1.linux.<arch>.rpm` | `scripts/package-linux.sh` |
| Linux 客户端（agent） | `telesh-agent-app_<ver>_linux_<arch>.deb`、`.AppImage` | `scripts/package-linux-app.sh` |
| Linux 客户端（server） | `telesh-server-app_<ver>_linux_<arch>.deb`、`.AppImage` | `scripts/package-linux-app.sh` |
| Windows 客户端 | `telesh-agent-app_<ver>_windows_x64-setup.exe`（NSIS）、`…_<locale>.msi`、`…_portable.zip`；server 同规则 | `scripts/package-windows.sh` |
| macOS 客户端 | `telesh-agent-app_<ver>_macos_<arch>.dmg`、`telesh-server-app_<ver>_macos_<arch>.dmg` | `scripts/package-mac.sh` |

命名约定：

- 服务包：`<crate>_<ver>-<rev>_<os>_<arch>`，`<os>` 固定 `linux`；
- 客户端：`<crate>-app_<ver>_<os>_<arch>`，`<os>` 取 `linux` / `windows` / `macos`；
- 架构按平台惯例：Linux `amd64`/`arm64`、Windows `x64`、macOS `x86_64`/`arm64`。

RPM 的 `Version` 不允许 `-`（semver 预发布必须带），`scripts/package-linux.sh`
会把它映射成 `~`（`0.1.0-rc.1` → `0.1.0~rc.1`），排序仍低于正式版。
GitHub Release 会把资产名里的 `~` 规范成 `.`；workflow 在上传前主动做同样
规范化，保证重新发布时能按同名覆盖。

流程：

```bash
# 1) 版本一致性（根 Cargo.toml / 两个 app 的 Cargo.toml + tauri.conf.json /
#    两个前端 package.json 必须一致）
./scripts/check-release-version.sh
# 2) 在 CHANGELOG.md 写 `## [<version>] - <date>`，并预览 Release 正文
./scripts/release-notes.sh 0.1.0
# 3) 打 tag 并推送（tag 必须等于 v<version>）
git tag v0.1.0 && git push origin v0.1.0
```

Release 正文来自 `CHANGELOG.md` 对应版本段（`scripts/release-notes.sh` 抽取）；
`verify` 阶段会先检查该版本条目存在，缺条目直接失败。发布前会删除同 tag 的
旧 release（含失败重跑留下的 draft），因此重跑 Release 工作流是幂等的。只想演练而不发 Release：
手动触发 `Release` 工作流，`publish=false`（可选 `with_mac=false`）。

CI 的构建机固定为 `ubuntu-22.04` / `windows-2022` / `macos-15`(+`macos-15-intel`)，
不使用 `-latest`：二进制动态链接 glibc，构建机版本决定产物的 glibc 下限
（22.04 = 2.35）。`Swatinem/rust-cache` 的 `key` 必须带镜像标签，避免跨镜像复用
`target/` 导致 `__isoc23_*` 未定义符号。

## 本机构建

前置：Rust、Node 22 / pnpm 10；Tauri 客户端还需要 `cargo install tauri-cli --locked`。

```bash
# 独立命令行工具包（Linux/macOS tar.gz、Windows zip；不依赖 Tauri）
./scripts/package-cli.sh              # 输出 dist/cli/

# headless deb + rpm（agent + server）
cargo install cargo-deb --locked
cargo install cargo-generate-rpm --locked
./scripts/package-linux.sh          # 输出 dist/linux/
SKIP_WEB_BUILD=1 ./scripts/package-linux.sh   # 前端已构建时

# Linux 桌面客户端 deb + AppImage
./scripts/package-linux-app.sh      # 输出 dist/linux-app/
# 受限网络 / 无 FUSE 时只出 deb：
LINUX_APP_TAURI_BUNDLES=deb ./scripts/package-linux-app.sh

# Windows（Git Bash / MSYS）
./scripts/package-windows.sh        # 输出 dist/windows/

# macOS（仅 macOS）
./scripts/package-mac.sh            # 输出 dist/mac/
```

Linux 客户端构建需要 Tauri 2 的系统依赖：`libwebkit2gtk-4.1-dev`、
`libayatana-appindicator3-dev`、`librsvg2-dev`、`libgtk-3-dev`、`libxdo-dev`、
`patchelf`。AppImage 由 Tauri 调 linuxdeploy/appimagetool 生成，需要能访问
GitHub 下载 runtime；CI 的 `ubuntu-22.04` 满足这些条件。

## macOS 签名与公证（可选）

不配置签名 secrets 时会产出**未签名** dmg（脚本打印 WARN），适合内部测试；
对外分发建议配置以下 GitHub secrets。`scripts/package-mac.sh` 会把它们映射成
Tauri 的 `APPLE_*` 变量，由 Tauri 完成临时钥匙串导入、hardened runtime 签名、
公证与钉票：

| secret | 说明 |
|---|---|
| `MAC_CERT_P12_BASE64` | `Developer ID Application` 证书及完整链的 p12（base64） |
| `MAC_CERT_PASSWORD` | p12 口令 |
| `MAC_SIGN_IDENTITY` | 签名身份全名，如 `Developer ID Application: <姓名> (<TeamID>)` |
| `MAC_NOTARY_API_KEY_ID` | App Store Connect API key id |
| `MAC_NOTARY_API_ISSUER_ID` | App Store Connect issuer id |
| `MAC_NOTARY_API_KEY_P8_BASE64` | API key 的 `.p8` 私钥（base64） |

`.p8` 只在构建时落到临时文件、用后即删，真实凭据不入仓库。

## 独立命令行工具包

`scripts/package-cli.sh` 不依赖 Tauri/GUI 栈，只编译 `telesh` 与 `telesh-server`
并打成压缩包；Linux/macOS 为 `tar.gz`，Windows 为 `zip`。每个包内只有：

- `telesh[.exe]` 或 `telesh-server[.exe]`
- `config.toml.example` / `server.toml.example`
- `LICENSE`
- `README.txt`

前端已经在编译期嵌入二进制，解压即可运行，不需要 Node 或单独的 `dist/` 目录。

```bash
tar -xzf telesh_0.1.0_linux_amd64.tar.gz
cd telesh_0.1.0_linux_amd64
./telesh --dataroot ./data
# 或
tar -xzf telesh-server_0.1.0_macos_arm64.tar.gz
cd telesh-server_0.1.0_macos_arm64
./telesh-server server.toml
```

Windows 解压 zip 后直接运行 `telesh.exe` / `telesh-server.exe`。

## Linux 服务包

安装内容：

- `/usr/bin/telesh`、`/usr/bin/telesh-server`
- `/usr/lib/systemd/system/telesh.service`、`telesh-server.service`
- 系统用户 / 组 `telesh`、`telesh-server`
- 数据目录 `/var/lib/telesh`、`/var/lib/telesh-server`
- 配置样例 `/usr/share/telesh/config.toml.example`、
  `/usr/share/telesh-server/server.toml.example`

安装：

```bash
sudo apt install ./telesh_0.1.0-1_linux_amd64.deb
sudo apt install ./telesh-server_0.1.0-1_linux_amd64.deb
# 或
sudo rpm -ivh telesh-0.1.0-1.linux.x86_64.rpm
sudo rpm -ivh telesh-server-0.1.0-1.linux.x86_64.rpm
```

`telesh-server` 安装后自动 `enable + start`，默认配置在
`/etc/telesh/server.toml`（首次安装生成，升级不覆盖），dashboard / 控制面在
`:9009`，边缘入口在 `:8080`。**agent 单元默认不启动**：先把
`/var/lib/telesh/config.toml` 里的 `control_server` 改成实际地址，再：

```bash
sudo systemctl enable --now telesh
```

运维：

```bash
systemctl status telesh telesh-server
journalctl -u telesh -f
journalctl -u telesh-server -f
```

卸载包不会删除 `/var/lib/telesh*`（数据库、身份、证书、ACME 缓存都在里面）。

## 桌面客户端

- Linux：`deb` 同时装主程序与 sidecar，依赖自动声明 webkit/gtk/appindicator；
  `AppImage` 单文件自带运行时，但 sidecar 仍在 AppImage 内。
- Windows：安装包把 `telesh-app.exe + telesh-sidecar.exe`（server 为
  `telesh-server-app.exe + telesh-server-sidecar.exe`）装到同一目录；便携 zip 直接把两个 exe 放在一起。未做代码签名，SmartScreen 可能提示
  「未知发布者」。MSI 的 `ProductVersion` 只接受数字，预发布版本会把第一个数字
  映射到 build 段（`0.1.0-rc.1` → `0.1.0.1`）。
- macOS：dmg 内是 Tauri 产出并已签名/公证的 `.app`，sidecar 在
  `Contents/MacOS/`。`.app` 由 Tauri 负责签名与钉票，DMG 由
  `scripts/package-mac.sh` 用 `hdiutil` 组装（带 Applications 链接与重试），
  避开 Tauri `bundle_dmg.sh` 在 CI 上偶发的 `Resource busy`。
  已签名时会做 `codesign --verify --deep --strict` 自检。
