import { create } from 'zustand'

async function api(path, options) {
  const res = await fetch(path, options)
  if (!res.ok) throw new Error(`${path}: HTTP ${res.status}`)
  return res.json()
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

  fetchAll: async () => {
    try {
      const { channel } = get()
      const q = channel ? `?channel=${encodeURIComponent(channel)}` : ''
      const [tunnels, requests] = await Promise.all([
        api('/api/tunnels'),
        api(`/api/requests${q}`),
      ])
      set({ tunnels, requests, loading: false, error: null, lastSync: Date.now() })
    } catch (e) {
      set({ error: errText(e), loading: false })
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
      if (!res.ok) throw new Error(`PUT /api/settings: HTTP ${res.status}`)
      set({ settings: await res.json() })
      return true
    } catch (e) {
      set({ error: errText(e) })
      return false
    }
  },

  /** 查某个账户当前提供的服务名(供「连接远程端口」选)。 */
  listServices: async (account) => {
    const res = await fetch(`/api/p2p/services?account=${encodeURIComponent(account.trim())}`)
    if (!res.ok) throw new Error(`GET /api/p2p/services: HTTP ${res.status}`)
    const data = await res.json()
    return Array.isArray(data.services) ? data.services : []
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
      if (!res.ok) throw new Error(`PUT /api/p2p-policy: HTTP ${res.status}`)
      set({ p2pPolicy: await res.json(), publishing: false })
      await get().fetchP2pPolicy()
      return true
    } catch (e) {
      set({ publishing: false, publishError: errText(e) })
      return false
    }
  },

  /**
   * Poll `/api/ports` and `/api/publish` independently so one failing
   * endpoint still lets the other section render.
   */
  fetchPublish: async () => {
    const [portsRes, pubRes] = await Promise.allSettled([
      api('/api/ports'),
      api('/api/publish'),
    ])

    const patch = {}
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
      if (!res.ok) throw new Error(`PUT /api/publish: HTTP ${res.status}`)
      const data = await res.json()
      set({ publish: Array.isArray(data) ? data : [], publishing: false })
      // "After a successful PUT, refresh."
      await get().fetchPublish()
      return true
    } catch (e) {
      set({ publishing: false, publishError: errText(e) })
      return false
    }
  },

  select: async (id) => {
    try {
      const detail = await api(`/api/requests/${id}`)
      set({ selected: detail })
    } catch (e) {
      set({ error: errText(e) })
    }
  },
}))
