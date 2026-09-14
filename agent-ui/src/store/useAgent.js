import { create } from 'zustand'

async function api(path) {
  const res = await fetch(path)
  if (!res.ok) throw new Error(`${path}: HTTP ${res.status}`)
  return res.json()
}

/**
 * Agent-local inspector state: tunnels (this agent), the rolling request
 * log, the currently inspected request, and replay output.
 */
export const useAgent = create((set, get) => ({
  loading: true,
  error: null,
  tunnels: [],
  requests: [],
  selected: null,
  replayOut: null,
  replaying: false,
  filters: { method: '', q: '' },

  setFilters: (f) => set({ filters: { ...get().filters, ...f } }),

  fetchAll: async () => {
    try {
      const [tunnels, requests] = await Promise.all([
        api('/api/tunnels'),
        api('/api/requests'),
      ])
      set({ tunnels, requests, loading: false, error: null })
    } catch (e) {
      set({ error: String(e?.message || e), loading: false })
    }
  },

  select: async (id) => {
    try {
      const detail = await api(`/api/requests/${id}`)
      set({ selected: detail, replayOut: null })
    } catch (e) {
      set({ error: String(e?.message || e) })
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