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
 * log, the currently inspected request, replay output, and the desired
 * port-publishing state (`/api/ports` + `/api/publish`).
 */
export const useAgent = create((set, get) => ({
  loading: true,
  error: null,
  lastSync: null,
  tunnels: [],
  requests: [],
  selected: null,
  replayOut: null,
  replaying: false,
  filters: { method: '', q: '' },

  // --- Ports & publishing -------------------------------------------------
  ports: [],
  portsError: null,
  publish: [],
  publishError: null,
  publishLoaded: false,
  publishing: false,
  lastSavedAt: null,

  setFilters: (f) => set({ filters: { ...get().filters, ...f } }),

  fetchAll: async () => {
    try {
      const [tunnels, requests] = await Promise.all([
        api('/api/tunnels'),
        api('/api/requests'),
      ])
      set({ tunnels, requests, loading: false, error: null, lastSync: Date.now() })
    } catch (e) {
      set({ error: errText(e), loading: false })
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
    await Promise.all([get().fetchAll(), get().fetchPublish()])
  },

  /** PUT the desired state; the response is the refreshed state array. */
  savePublish: async (desired) => {
    set({ publishing: true, publishError: null })
    try {
      const res = await fetch('/api/publish', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(desired),
      })
      if (!res.ok) throw new Error(`PUT /api/publish: HTTP ${res.status}`)
      const data = await res.json()
      set({
        publish: Array.isArray(data) ? data : [],
        publishing: false,
        lastSavedAt: Date.now(),
      })
      // "After a successful PUT, refresh."
      await get().fetchPublish()
    } catch (e) {
      set({ publishing: false, publishError: errText(e) })
    }
  },

  select: async (id) => {
    try {
      const detail = await api(`/api/requests/${id}`)
      set({ selected: detail, replayOut: null })
    } catch (e) {
      set({ error: errText(e) })
    }
  },

  replay: async (id) => {
    const { selected } = get()
    set({ replaying: true })
    try {
      const res = await fetch(`/api/requests/${id}/replay`, { method: 'POST' })
      const text = await res.text()
      set({ replayOut: `${new Date().toLocaleTimeString()} — HTTP ${res.status}${text ? `\n${text}` : ''}`, replaying: false })
      // refresh the log (the replay may have produced a new real request too)
      const requests = await api('/api/requests')
      const fresh = requests.find((r) => r.id === selected?.id) || null
      set({ requests, selected: fresh })
    } catch (e) {
      set({ replayOut: `replay failed: ${e?.message || e}`, replaying: false })
    }
  },
}))
