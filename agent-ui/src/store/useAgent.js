import { create } from 'zustand'

/** 探测自身超时(ms)。 */
const PROBE_TIMEOUT = 2000

/**
 * 同一轮轮询里的多次失败只探一次(间隔小于它就复用上一次的结果)。
 *
 * 为什么是这个值:轮询是 2s 一轮,同一轮里 `fetchAll` 与 `fetchPublish` 会各失败一次,
 * 500ms 足够把它们合并掉;而跨轮询必须重新探 —— 否则一次陈旧的「不可达」会把真正的
 * 接口错误误判成「面板没在跑」。
 */
const PROBE_MIN_GAP = 500

let lastProbeAt = 0
let lastProbeUp = false
let inflightProbe = null

/**
 * 面板还活着吗:`GET /api/instance` 认指纹(`app === 'telesh'`),跟 telesh 自己
 * 判断「这个端口上是 telesh」用的是同一条判据。
 *
 * 只在**失败**时问它 —— 成功路径上一律不探。
 */
function probeAgent() {
  if (inflightProbe) return inflightProbe
  if (lastProbeAt !== 0 && Date.now() - lastProbeAt < PROBE_MIN_GAP) {
    return Promise.resolve(lastProbeUp)
  }
  inflightProbe = (async () => {
    const ctrl = new AbortController()
    const timer = setTimeout(() => ctrl.abort(), PROBE_TIMEOUT)
    let up = false
    try {
      const res = await fetch('/api/instance', { cache: 'no-store', signal: ctrl.signal })
      const info = await res.json()
      up = res.ok && info?.app === 'telesh'
    } catch {
      up = false
    } finally {
      clearTimeout(timer)
      lastProbeAt = Date.now()
      lastProbeUp = up
      inflightProbe = null
    }
    return up
  })()
  return inflightProbe
}

/** 接口错误:带上状态码,好判断该不该去探面板。 */
class ApiError extends Error {
  constructor(path, status) {
    super(`${path}: HTTP ${status}`)
    this.status = status
  }
}

async function api(path, options) {
  const res = await fetch(path, options)
  if (!res.ok) throw new ApiError(path, res.status)
  return res.json()
}

/**
 * 失败归类:true = 面板不可达(根因不在这个接口上)。
 *
 * - `fetch` 抛错:请求根本没拿到响应 —— 面板没在跑(生产/托盘 app 下就是这种);
 * - 4xx:服务端答了,是这个接口自己的问题,不用探;
 * - 5xx:可能是面板没在跑(dev 下 vite 的 `/api` 代理回的就是 500),探一次。
 */
async function isAgentDown(e) {
  if (!(e instanceof ApiError)) return true
  if (e.status < 500) return false
  return !(await probeAgent())
}

function errText(e) {
  return String(e?.message || e)
}

/**
 * Agent-local inspector state: tunnels (this agent), the rolling request
 * log, the currently inspected request, and the desired port-publishing state
 * (`/api/ports` + `/api/publish`).
 */
export const useAgent = create((set, get) => ({
  loading: true,
  error: null,
  /** 面板不可达:由 `/api/instance` 探测决定,全局状态条只用这一个标志。 */
  agentDown: false,
  /** 失败的一次操作(发布/规则/设置):只给 toast 用,不在页面上重复显示。 */
  actionError: null,
  lastSync: null,
  tunnels: [],
  requests: [],
  channels: [],
  channel: '',
  account: null,
  selected: null,
  filters: { method: '', q: '' },

  setChannel: (channel) => set({ channel }),

  // --- Ports & publishing -------------------------------------------------
  ports: [],
  portsError: null,
  publish: [],
  publishError: null,
  publishLoaded: false,
  publishing: false,
  p2pPolicy: null,
  settings: { publish: {}, p2p: {} },

  setFilters: (f) => set({ filters: { ...get().filters, ...f } }),

  /**
   * 所有失败都收口到这里。
   *
   * 根因是「面板不可达」就只翻 `agentDown`,并清掉功能级错误 —— 由 App 顶部那
   * 一条状态条统一说一次,各个页面不再各报一遍同样的话;面板活着才把错误交给具
   * 体功能,由它自己决定显示在哪(toast / 对话框里那行)。
   *
   * 返回 true = 已按「面板不可达」处理。
   */
  noteFailure: async (e) => {
    if (await isAgentDown(e)) {
      set({ agentDown: true, error: null, publishError: null, portsError: null })
      return true
    }
    set({ agentDown: false })
    return false
  },

  fetchAll: async () => {
    try {
      const { channel } = get()
      const q = channel ? `?channel=${encodeURIComponent(channel)}` : ''
      const [tunnels, requests] = await Promise.all([
        api('/api/tunnels'),
        api(`/api/requests${q}`),
      ])
      set({
        tunnels,
        requests,
        loading: false,
        error: null,
        agentDown: false,
        lastSync: Date.now(),
      })
    } catch (e) {
      set({ loading: false })
      if (!(await get().noteFailure(e))) set({ error: errText(e) })
    }
  },

  /** 有日志的通道列表(用于筛选)。 */
  fetchChannels: async () => {
    try {
      set({ channels: await api('/api/channels') })
    } catch {
      set({ channels: [] })
    }
  },

  /** 本机 agent 的登录状态与账户信息(失败时不影响其它数据)。 */
  fetchAccount: async () => {
    try {
      set({ account: await api('/api/account') })
    } catch {
      set({ account: null })
    }
  },

  /** P2P 入站规则(哪些本地端口允许被远端 P2P 直连)。 */
  fetchP2pPolicy: async () => {
    try {
      set({ p2pPolicy: await api('/api/p2p-policy') })
    } catch {
      set({ p2pPolicy: null })
    }
  },

  /** 端口设置(publish / p2p 分开,默认全开)。 */
  fetchSettings: async () => {
    try {
      set({ settings: await api('/api/settings') })
    } catch {
      set({ settings: { publish: {}, p2p: {} } })
    }
  },

  /** 覆盖式提交端口设置(立即生效);返回是否成功。 */
  applySettings: async (settings) => {
    try {
      const res = await fetch('/api/settings', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(settings),
      })
      if (!res.ok) throw new ApiError('PUT /api/settings', res.status)
      set({ settings: await res.json() })
      return true
    } catch (e) {
      if (!(await get().noteFailure(e))) set({ error: errText(e) })
      return false
    }
  },

  /** 查某个账户当前提供的服务名(供「连接远程端口」选)。面板不可达时返回空。 */
  listServices: async (account) => {
    try {
      const data = await api(`/api/p2p/services?account=${encodeURIComponent(account.trim())}`)
      return Array.isArray(data.services) ? data.services : []
    } catch (e) {
      if (await get().noteFailure(e)) return []
      throw e
    }
  },

  /** 覆盖式提交 P2P 规则(即时生效);返回是否成功。 */
  applyP2pPolicy: async (policy) => {
    set({ publishing: true, publishError: null })
    try {
      const res = await fetch('/api/p2p-policy', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(policy),
      })
      if (!res.ok) throw new ApiError('PUT /api/p2p-policy', res.status)
      set({ p2pPolicy: await res.json(), publishing: false })
      await get().fetchP2pPolicy()
      return true
    } catch (e) {
      set({ publishing: false })
      if (!(await get().noteFailure(e))) set({ actionError: errText(e) })
      return false
    }
  },

  /**
   * 轮询 `/api/ports` 与 `/api/publish`(哪个失败都不影响另一个渲染)。
   *
   * 面板不可达时只翻 `agentDown`(全局状态条负责说话),不写 `portsError` /
   * `publishError` —— 否则同一个根因会在页面上重复三遍。
   */
  fetchPublish: async () => {
    const [portsRes, pubRes] = await Promise.allSettled([
      api('/api/ports'),
      api('/api/publish'),
    ])

    const failed = [portsRes, pubRes].find((r) => r.status === 'rejected')
    if (failed && (await get().noteFailure(failed.reason))) {
      set({ ports: [], portsError: null, publishError: null })
      return
    }

    const patch = { agentDown: false }
    if (portsRes.status === 'fulfilled') {
      patch.ports = Array.isArray(portsRes.value) ? portsRes.value : []
      patch.portsError = null
    } else {
      patch.ports = []
      patch.portsError = errText(portsRes.reason)
    }
    if (pubRes.status === 'fulfilled') {
      patch.publish = Array.isArray(pubRes.value) ? pubRes.value : []
      patch.publishError = null
      patch.publishLoaded = true
    } else {
      patch.publishError = errText(pubRes.reason)
    }
    set(patch)
    if (patch.portsError == null && patch.publishError == null) set({ lastSync: Date.now() })
  },

  refreshAll: async () => {
    await Promise.all([
      get().fetchAll(),
      get().fetchPublish(),
      get().fetchAccount(),
      get().fetchP2pPolicy(),
      get().fetchSettings(),
      get().fetchChannels(),
    ])
  },

  /** 覆盖式提交期望状态(即时生效);返回是否成功。 */
  applyPublish: async (desired) => {
    set({ publishing: true, publishError: null })
    try {
      const res = await fetch('/api/publish', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(desired),
      })
      if (!res.ok) throw new ApiError('PUT /api/publish', res.status)
      const data = await res.json()
      set({ publish: Array.isArray(data) ? data : [], publishing: false })
      // "After a successful PUT, refresh."
      await get().fetchPublish()
      return true
    } catch (e) {
      set({ publishing: false })
      if (!(await get().noteFailure(e))) set({ actionError: errText(e) })
      return false
    }
  },

  select: async (id) => {
    try {
      const detail = await api(`/api/requests/${id}`)
      set({ selected: detail })
    } catch (e) {
      if (!(await get().noteFailure(e))) set({ error: errText(e) })
    }
  },
}))
