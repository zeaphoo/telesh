# agent-ui / server-ui 视觉与交互设计(v2)

状态:**设计稿(未实现)**。范围仅限两个前端(`agent-ui` 本机 inspector、
`server-ui` 服务端 dashboard),不动控制面/数据面协议。

## 0. 已定技术选型(本轮)

| 项 | 决策 |
|---|---|
| UI 基座 | **shadcn/ui 默认组件与默认样式**(`style: new-york`,`baseColor: neutral`,CSS 变量) |
| 主色 | shadcn `neutral`(黑白灰);品牌色仅作为未来的 preset,现在不做 |
| 主题 | 现在实现 **light / dark / system** 切换;**预留多主题(preset)扩展点** |
| i18n | **中文(zh-CN)+ 英文(en)**,`i18next` + `react-i18next`,静态打包;**浏览器语言优先,回退 `zh-CN`** |
| 图表 | **ECharts**(`echarts` + `echarts-for-react`);**不用 shadcn `chart`**(它基于 recharts) |
| 组织 | **不引入 pnpm workspace,两个 app 完全独立**;主题 token、i18n、shadcn 组件各自持有 |
| 动效 | 跟随 shadcn 默认(`tw-animate-css`),只加 `prefers-reduced-motion` 兜底 |

---

## 1. 现状审计

### 1.1 已确认的问题

| 问题 | 位置 | 处置 |
|---|---|---|
| 完全没有深色模式 | `*/src/index.css` | §4 |
| 颜色硬编码/不一致:`emerald-100`/`red-100`、`--primary` 紫而 `--ring` 蓝、无语义状态 token | `badge.jsx`、`index.css` | §4 §5 |
| 只有 4 个裁剪版组件(无 Input/Select/Tabs/Dialog/Toast…) | `components/ui/` | §3 用 CLI 补全 |
| 两个 app 的设计系统逐字节重复 | 两个 `src/` | §7 接受重复,各自演进 |
| agent-ui 中英混排;错误显示裸 `HTTP 500` | `PublishView`/`RequestLogView`/store | §6 |
| 无骨架屏 / toast / 空状态 / 连接状态 | — | §8 §9 |
| 无 favicon / `theme-color`,`<title>` 小写 | `index.html` | §10 |

### 1.2 信息架构与交互

server-ui 顶部只有 3 个文字链接(无图标/品牌/主题/同步状态),agent-ui 把 tab 挤进 header;
无侧边栏与移动端抽屉,`max-w-6xl` 固定导致窄屏表格溢出;轮询 2s/3s 全量重取,整表重渲染
且滚动跳动、不感知标签页隐藏。

---

## 2. 设计原则

1. **shadcn 默认优先**。默认组件、默认样式、默认 token;要改先改 token,再考虑 wrapper,
   最后才改生成出来的组件源码。
2. **深色与浅色等价**(两套 token,不留硬编码颜色);**主题可演进**(模式与 preset 分层,preset 留空接口)。
3. **零硬编码文案**。所有可见字符串走 `t()`,含 `title`、`aria-label`、空状态、错误。
4. **每个异步动作四态**:loading / empty / error / success。
5. **状态有冗余表达**(颜色 + 圆点/图标 + 文案),**动效克制且可关**。

---

## 3. shadcn/ui 接入

### 3.1 前置(两个 app 各做一次)

```bash
# 1) 依赖(shadcn 需要的都在这里;v4 用 tw-animate-css 取代 tailwindcss-animate)
pnpm add class-variance-authority clsx tailwind-merge lucide-react \
         tw-animate-css sonner
pnpm add -D @types/node        # vite.config 里用 path.resolve
```

```js
// vite.config.js —— 关键：@ 别名，shadcn 生成的组件统一按别名导入
import { fileURLToPath, URL } from 'node:url'
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  // ...
})
```

> 这两个 app 都是 ESM(`"type": "module"`),用 `import.meta.url` 而**不是** `__dirname`。

```json
// jsconfig.json（JS 项目也需要，编辑器与 CLI 靠它解析别名）
{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }
```

### 3.2 `components.json`(CLI 生成,不要手改)

CLI 4.x 用 **preset** 决定字体/配色。本项目选:`-b radix`(Radix 组件库)+
`-p nova`(= neutral 配色 + Lucide 图标 + Geist 字体)。生成结果(节选):

```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "radix-nova",
  "rsc": false,
  "tsx": false,
  "tailwind": { "config": "", "css": "src/index.css",
                "baseColor": "neutral", "cssVariables": true, "prefix": "" },
  "iconLibrary": "lucide",
  "aliases": { "components": "@/components", "utils": "@/lib/utils",
               "ui": "@/components/ui", "lib": "@/lib", "hooks": "@/hooks" },
  "menuColor": "default",
  "menuAccent": "subtle",
  "registries": {}
}
```

### 3.3 `src/index.css` 结构(Tailwind v4 + shadcn 默认)

```css
@import "tailwindcss";
@import "tw-animate-css";
@import "shadcn/tailwind.css";          /* shadcn 4.x 基础层(依赖 shadcn 包) */
@import "@fontsource-variable/geist";   /* preset 自带字体,本地打包(离线可用) */

@custom-variant dark (&:is(.dark *));

/* 依次:  :root(light token) -> .dark(深色覆盖) -> @theme inline -> @layer base
   再加上 telesh 扩展色(--success/--warning/--info/--kind-*) */
```

**token 直接写在 `index.css` 里**(不拆 `lib/theme.css`)—— 这是 shadcn 的默认单文件布局,
少一个可能被 CLI 改动的跳转。

### 3.4 组件清单(CLI 生成,默认样式)

```bash
# 4.x 用 preset;这里 radix 组件库 + nova preset
npx shadcn@latest init -b radix -p nova --no-monorepo -y
npx shadcn@latest add -y -o \
  button card badge input label select textarea tabs separator skeleton \
  tooltip dialog alert-dialog dropdown-menu switch sheet sonner table \
  alert scroll-area
# 图表不用 shadcn chart（基于 recharts）；telesh 用 ECharts（P3 再装）
```

CLI 4.x 实际带入的依赖(比旧版多):

| 依赖 | 用途 |
|---|---|
| `radix-ui` | 统一包,**不再是一堆 `@radix-ui/react-*`** |
| `cn` | 新的 `cn()` 实现(`lib/utils.js` 只是 `export { cn } from "cn"`) |
| `shadcn` | 提供 `shadcn/tailwind.css` 基础层 |
| `next-themes` | **sonner 组件依赖它**(所以我们自己也用它做主题,见 §4.1) |
| `@fontsource-variable/geist` | preset 字体,本地打包 |
| `tw-animate-css` | Tailwind v4 动画(取代 `tailwindcss-animate`) |

- 现有 `button/card/badge/table` 被 CLI **覆盖**为官方版本。
- `lib/utils.js` 也被覆盖为纯 `cn`;业务格式化另放 `lib/format.js`(见 §7)。
- `toast` 用 **sonner**;移动端抽屉用 `sheet`;确认弹窗用 `alert-dialog`;行内横幅用 `alert`。

### 3.5 覆盖策略(保证"默认"不被侵蚀)

```
token 不够  →  加 CSS 变量(index.css)
变体不够  →  app 内 wrapper / cva 变体(如 StatusBadge、StatCard)
必须改生成文件 →  在文件头注释写明原因，便于以后对照上游
```

自定义业务组件(**不**从 CLI 生成,放在 `src/components/`):

`StatusDot`、`StatusBadge`、`StatCard`、`EmptyState`、`CopyButton`、`LiveIndicator`、
`PageHeader`、`LocaleSwitcher`、`ThemeToggle`、`TrafficChart`(P3)。

---

## 4. 主题系统

### 4.1 模式(现在实现)

用 **next-themes**(shadcn 的 sonner 组件依赖它,所以必然要装,顺势用它做主题):

- `main.jsx` 里用 `ThemeProvider` 包住 App,参数:`attribute="class"`(匹配
  `@custom-variant dark (&:is(.dark *))`)、`storageKey="telesh-ui-theme"`、
  `defaultTheme="system"`、`enableSystem`、`disableTransitionOnChange`。
- `<ThemeToggle>` 用 shadcn `dropdown-menu` + `useTheme()`,三态:浅色 / 深色 / 跟随系统。
- **防闪白**:`index.html` 的 `<head>` 内联 ~10 行脚本,在首帧前按**同一个 localStorage key**
  写好 `.dark` 与 `color-scheme`。

```html
<script>
  (function () {
    try {
      var t = localStorage.getItem('telesh-ui-theme') || 'system';
      var d = t === 'dark' || (t === 'system' &&
        matchMedia('(prefers-color-scheme: dark)').matches);
      document.documentElement.classList.toggle('dark', d);
      document.documentElement.style.colorScheme = d ? 'dark' : 'light';
    } catch (e) {}
  })();
</script>
```

### 4.2 token 分层(为"未来主题切换"预留)

```
第 1 层 原语(不直接用)      --gray-50 …、--teal-500 …      ← 现在不建，保持 shadcn 默认
第 2 层 语义(组件只读这层)   --background --foreground --primary --ring
                            --muted --border --card --popover --destructive
                            --chart-1..5 --sidebar-* (shadcn 默认已含)
第 3 层 扩展语义(telesh 加的) --success/--warning/--info(+ -foreground)
                            --kind-http/--kind-tcp/--kind-p2p/--kind-mirror
第 4 层 组件覆写             --radius、状态底色
```

第 2 层完全用 shadcn 默认(`neutral`,即浅色近黑 / 深色近白的主色,`--ring` 自动跟随),
**紫色/蓝色不一致的问题因此消失**。

### 4.3 面向未来的主题 preset

模式与调色板解耦,preset 只是一段覆盖第 2/3 层的 CSS:

```css
/* [data-theme-preset="violet"] { --primary: oklch(...); --ring: oklch(...); } */
/* [data-theme-preset="high-contrast"] { ... } */
```

现在:属性不设置、不加任何 preset 块,只保证所有组件引用的是 CSS 变量(本来就是),
以后加 preset **不需要动任何组件代码**。`applyTheme()` 预留 `setThemePreset(name)` 空实现。

### 4.4 深色下的层次

shadcn 默认已用 `border` + 半透明 `ring` 表达层次,补一条约定:
深色下**以边框为主、阴影为辅**;弹层用 `--popover` + `shadow-lg`。浅色反之。
不额外发明阴影(token 里只保留 `--radius`)。

### 4.5 延伸色(shadcn 之外,加在 `index.css` 第 3 层)

```
状态(红绿黄蓝之外的中性态)： --success / --warning / --info  (+ -foreground)
隧道类型：--kind-http(青) --kind-tcp(琥珀) --kind-p2p(紫) --kind-mirror(中性)
```

深浅两套都要给,对比度 ≥4.5:1。`chart-1..5` 直接用 shadcn 默认。

---

## 5. 状态语义(两 app 语义一致,各自实现)

```
live / online          → success   ● 实心圆
pending / starting     → warning   ◐ 呼吸动画
error                  → danger    ▲
target_gone / 未监听   → muted     ○
none / 未发布          → muted     —
```

`src/lib/status.js` 导出 `statusMeta(state)` → `{ variant, dot, i18nKey }`;`<StatusBadge state>`
用 shadcn `Badge` 的 `variant` + 自定义 `--*-soft` 底色;文案走 i18n,不写死。

---

## 6. i18n

### 6.1 选型

`i18next` + `react-i18next`,**静态打包**(内置 resources,无 HTTP backend,
Rust 二进制内嵌/离线场景必须如此)。理由:生态标准、支持插值/复数/命名空间、
`useTranslation` 在 React 19 下工作良好;分语言包按需懒加载留给以后。

### 6.2 文件组织

每个 app 自己一份(完全独立,不跨目录 import):

```
agent-ui/src/i18n/            # server-ui 同构
  index.js                    # createI18n()：检测、持久化、useLocale
  locales/zh-CN/common.json   # 状态/类型/单位/通用动作/错误码
  locales/zh-CN/agent.json    # 本 app 页面文案(namespace: agent)
  locales/en/common.json
  locales/en/agent.json
```

- `common`:两个 app 语义一致(术语表见 §6.5),但**各存一份**,允许独立演进。
- 默认命名空间:app 自己的(`agent` / `server`);`common` 显式 `t('common:status.live')`。

### 6.3 键规范

```
<namespace>.<区块>.<元素>[.<细节>]
common.status.live            common.kind.p2p
common.action.saveApply       common.error.http
agent.publish.title           agent.publish.columns.remotePort
server.overview.agentsOnline  server.tunnels.empty
```

同一概念在两个 app 里必须用同一个 `common` 键(术语统一,见 §6.5)。

### 6.4 运行时行为

- **检测顺序**:localStorage `telesh-ui-locale` → `navigator.language`
  (`zh*` → `zh-CN`,否则 `en`) → `fallbackLng`。
- **浏览器语言优先**;无法判定时回退 `zh-CN`(`fallbackLng: 'zh-CN'`,项目文档即中文)。
- 切换语言:同步 `<html lang>`、`document.title`(用 `t()` 设置)、并触发重渲染。
- **格式化用当前 locale**:`Intl.NumberFormat` / `Intl.DateTimeFormat`;
  `formatBytes` 单位保留 B/KB/MB…,数字按 locale;`toLocaleTimeString(locale)`。
- **错误码 → i18n**:store 里**不存裸字符串**,存 `{ code, params }`
  (如 `{code:'http', params:{status:500}}`),UI 用
  `t('common.error.' + code, params)`。后端返回的 `detail` 若为已知枚举,映射成
  `common.detail.<code>`;未知原样显示并保留非本地化标记。
- **无障碍文案也翻译**:`aria-label`、`Tooltip`、`Dialog` 的 sr-only「关闭」等
  (shadcn 组件里的英文 sr-only 文案通过 props 传 `t()`)。

### 6.5 术语表(中英对照,两 app 共用)

| key | zh-CN | en |
|---|---|---|
| `common.kind.http` | HTTP | HTTP |
| `common.kind.tcp` | TCP | TCP |
| `common.kind.p2p` | P2P | P2P |
| `common.kind.mirror` | 镜像 | Mirror |
| `common.status.live` | 运行中 | Live |
| `common.status.pending` | 生效中 | Pending |
| `common.status.error` | 错误 | Error |
| `common.status.targetGone` | 目标已消失 | Target gone |
| `common.status.notListening` | 未监听 | Not listening |
| `agent.nav.publish` | 端口与发布 | Ports & publishing |
| `agent.nav.log` | 请求日志 | Request log |
| `common.action.saveApply` | 保存并应用 | Save & apply |
| `common.action.replay` | 重放请求 | Replay request |

### 6.6 防遗漏

- 每个 app 内 `zh-CN` 与 `en` 的键集合尽量保持一致(漏译会在界面直接显示 key)。
- dev 期开启 `i18next` 的 missing-key 警告;`saveMissing` 关闭(不写文件)。

---

## 7. 代码组织(两个 app 完全独立)

**不引入 pnpm workspace**:两个 app 各自 `pnpm install` / `pnpm build` / 各自产出 `dist/`,
没有任何跨目录 import,也不共享任何 npm 包。`agent-ui` 与 `server-ui` 是平行、互不依赖的
两个前端(只是刻意保持结构一致,便于对照)。

每个 app 内部结构相同:

```
agent-ui/  (server-ui 同构)
  components.json                # shadcn 配置(style/baseColor/别名)
  jsconfig.json                  # "@/*" -> "./src/*"
  vite.config.js                 # 同一套 @ 别名
  src/
    index.css                    # tailwindcss + tw-animate-css + shadcn/tailwind.css + 字体;token 在文件内
    lib/
      utils.js                   # shadcn 生成:`export { cn } from "cn"`
      format.js                  # formatBytes / formatDuration
      status.js                  # statusMeta()(P1)
    i18n/
      index.js                   # initI18n：检测语言、持久化、同步 <html lang>
      locales/{zh-CN,en}/{common,<app>}.json
    components/
      ui/                        # shadcn 生成,尽量不改
      theme-toggle.jsx  locale-switcher.jsx
      ...                        # StatusBadge / StatCard / EmptyState / CopyButton …(P1)
    pages/ | views/
```

**关于重复**:主题 token 与 `common` 文案在两个 app 里各存一份。这是刻意的 —— 两个 app
独立演进,不做任何自动校验或同步,允许各自修改视觉与文案。只是新增/改一个 token 或
`common` 键时,记得两边都改(否则一边会静默回退成默认色、或界面直接显示 key 名)。

---

## 8. server-ui 重构

### 8.1 外壳

导航在顶部 navbar 上,**没有侧边栏**。

```
┌────────────────────────────────────────────────────────────────────┐
│ [icon] telesh   Overview  Clients  Tunnels   ● 已连接  ⟳  ☾  [中/EN] │  56px
├────────────────────────────────────────────────────────────────────┤
│  content(居中，24px padding)                                       │
└────────────────────────────────────────────────────────────────────┘
```

导航项:lucide 图标 + 文案,active 用 `bg-accent text-accent-foreground`;
窄屏(<sm)只显示图标,`aria-label` 保留完整名称。导航文案 `t('<app>.nav.*')`。
移动端不再用 `sheet`(仅 2–3 项,横排够用)。

### 8.2 Overview

- 顶部 4 张 `StatCard`(在线 agent / 隧道 / 活跃连接 / HTTP 请求),
  数字 `tabular-nums`,`agents_online === 0` 时用 warning 色。
- **流量曲线**:ECharts(`echarts` + `echarts-for-react`)。主题随 `data-theme` 切换
  (注册 light/dark 两套 option,颜色取 `--chart-1..5`);v1 用客户端环形缓冲存最近 N 次
  采样,后端时间序列见 §11。
- Live tunnels:状态圆点 + 类型徽标 + 等宽 `remote` + `CopyButton` + 迷你流量条;
  只留最近 5 条 + 「查看全部」(去掉与 Tunnels 页的重复)。

### 8.3 Clients

Subdomain(`Badge` + copy)、Agent ID(等宽截断 + `Tooltip` + copy)、隧道数、
在线时长、`StatusDot`。只读,不做 kick。

### 8.4 Tunnels

Type(类型色 `Badge`)、Public entry(等宽 + copy)、In/Out(右对齐 + 迷你条)、
`StatusBadge`。表头粘性;空状态用 `EmptyState`。

### 8.5 全局

- `PageHeader`(标题 + 描述 + 右侧刷新)。
- 首屏 `Skeleton`;轮询按 key diff,**不整表替换**;`document.hidden` 时暂停。
- 错误:`sonner` toast(瞬时)+ `alert`(持续)。

---

## 9. agent-ui 重构

### 9.1 外壳

与 server-ui 同款外壳(导航在顶部 navbar,无侧边栏),导航两项(端口与发布 / 请求日志),
品牌区为 `SquareTerminal` 图标 + “telesh”;连接状态与隧道信息放在 navbar 右侧。

### 9.2 端口与服务(即时生效,无保存按钮)

页面分两个区块、对应三个操作;所有操作**立即 PUT** 生效,无“保存并应用”/dirty/`beforeunload`。

- **区块一 · 对外发布**:本机端口发到公网(HTTP 子域 / 公网 TCP)。未配置时只显示一句空态,
  **不渲染空表格**;有内容时列 端口 / 目标 / 状态 / 操作。新增走对话框(排除已发布端口)。
- **区块二 · 远程连接与 P2P**:
  - *连接远程端口*:填 `ACCOUNT/NAME` + 本地端口,立即生效(镜像)。
  - *P2P 入站规则*:`全部端口 / 选择端口 / 不允许`,决定哪些本地端口可被远端 P2P 直连
    (服务名 = 端口号);存 `<dataroot>/p2p.json`,daemon 展开成 P2P 条目(见 §11)。
- 顶部仅在有配置时显示一行概览(已发布 N / 镜像 N / P2P 模式),否则整行隐藏。

### 9.3 请求日志

- 左日志右详情;日志表加 status 圆点、时长色条、字节数右对齐。
- **暂停/继续 live tail** 开关(`switch`)+ `Kbd` 提示。
- 键盘 `j/k` 选择、`r` replay、`/` 聚焦过滤。
- 详情用 `tabs`(Headers / Body),JSON 格式化 + 折叠 + `CopyButton`;
  空内容用 `EmptyState`,不显示 `(empty)`。
- Replay:按钮 loading + 结果 toast,内联结果面板保留。

### 9.4 账户与登录状态

后端 `crates/telesh/src/account.rs` 保存 `AccountInfo` 快照(`daemon` 重连/登录/断线时更新;
控制服务器、数据目录、公钥、本机节点名启动时写入),新增 `GET /api/account`。节点名缺失时
`state::load` 会补一个 LocalSend 风格的默认名并落盘,服务端没返回显示名则回退本机名;
账号句柄按 `000-012-345` 每 3 位加短横线显示。UI:navbar 只保留一个状态圆点,显示
`账号 – 显示名`(否则状态文案),数值区定宽 + `tabular-nums` 避免抖动;另有“账户”页列出
登录状态、账户、显示名、控制服务器、身份公钥(可复制)、会话过期、最近登录、数据目录与最近错误。

### 9.5 语言

**中文默认 + EN 切换**(§6.4),`LocaleSwitcher` 用 `dropdown-menu`/`select`;中英混排已消除。

---

## 10. 无障碍、动效与元信息

- 所有可交互元素 `:focus-visible` 用 `--ring`;Tabs 用 Radix roving focus,`sheet`/`dialog` 自带焦点陷阱与 `aria-modal`。
- 表格 `<th scope="col">`;数字列 `tabular-nums`。
- 状态除颜色外加圆点/图标/文案;同步与保存结果用 `aria-live="polite"`。
- `prefers-reduced-motion: reduce` 下把 `tw-animate-css` 的动画降为 0。
- `index.html`:加 favicon、`theme-color`(深浅各一)、i18n 同步 `lang`、`t()` 设 `<title>`。

---

## 11. 后端/接口配合(小改)

| 项 | 说明 | 优先级 |
|---|---|---|
| ~~`GET /api/account`~~ ✅ | agent 的登录状态 + 账户信息(§9.4),已实现 | — |
| ~~`GET/PUT /api/p2p-policy`~~ ✅ | P2P 入站规则(§9.2):存 `p2p.json`,daemon 展开成 P2P 条目 | — |
| 静态资源缓存头 | hash 资源 `Cache-Control: immutable`,index.html `no-cache` | P1 |
| `/api/healthz` 复用 | 用作"已连接"指示(server 已有) | P1 |
| 错误码化 | 后端返回稳定 `code` 而非可读句子,便于 i18n | P2 |
| 构建信息 | `/api/meta` 返回版本/构建时间,用于页脚 | P2 |
| 指标时间序列 | `/api/metrics/history`(或客户端环形缓冲先行) | P2 |
| 请求流式 | SSE `GET /api/requests/stream` 取代 2s 轮询 | P3 |

---

## 12. 分阶段落地

> **已完成**:P0(shadcn 4.x、主题、i18n)、P1(navbar 外壳、基础组件、文案抽取)、
> P2(agent-ui 即时发布 + 发布对话框;server-ui 骨架屏)、P3 分项(账户/登录状态、端口与服务改版)。
> 验证:两个 app `pnpm build`;无头 Chromium 根/子路由 × zh/en × 浅/深色;CDP 真实点击。

| 阶段 | 内容 | 产出 |
|---|---|---|
| ~~P0 接入~~ ✅ | 两个 app 各自接 shadcn CLI(§3)、主题(§4)、i18n 骨架(§6) | 深色/浅色可切、shadcn 默认外观、i18n 通道打通 |
| ~~P1 骨架与组件~~ ✅ | 外壳/navbar 导航、`StatusBadge`/`StatCard`/`EmptyState`/`CopyButton`/`LiveIndicator`/`PageHeader`、全部文案抽取 | 结构统一、无硬编码文案 |
| ~~P2 页面重构~~ ✅ | §8 三页 + §9 两页 + 骨架屏 + 错误/空/成功四态 | 业务页焕新 |
| **P3 体验** | live tail 时间轴/高亮、ECharts 流量曲线、移动端细节 | 手感 |
| **P4 收尾** | reduced-motion、favicon/`theme-color`、缓存头、a11y 全量、代码分割(server-ui 单 chunk 已 >500KB) | 可交付 |

每阶段结束:`pnpm -C <app> build`;dev 页 `/__kitchen-sink` 在 light/dark × zh/en 下目检。

---

## 13. 决策记录

已定:

1. **主色** = shadcn `neutral`(黑白灰);品牌色仅作为未来的 preset,现在不做。
2. **默认语言** = 浏览器语言优先,无法判定时回退 `zh-CN`。
3. **图表** = ECharts(`echarts` + `echarts-for-react`),不用 shadcn `chart`。
4. **代码组织** = 不引入 pnpm workspace,两个 app 完全独立,不做一致性校验。

待确认(小):回退语言用 `zh-CN` 还是 `en`?(浏览器有 `zh` 就走中文,回退只影响其他语种用户)