import { create } from 'zustand'

async function api(path) {
  const res = await fetch(path)
  if (!res.ok) throw new Error(`${path}: HTTP ${res.status}`)
  return res.json()
}

/**
 * Global dashboard state. `fetchAll` hits the three JSON endpoints
 * (clients / tunnels / metrics) and is driven by a 3s poll in App.
 */
export const useDashboard = create((set) => ({
  loading: true,
  error: null,
  clients: [],
  tunnels: [],
  metrics: null,
  lastSync: null,

  fetchAll: async () => {
    try {
      const [clients, tunnels, metrics] = await Promise.all([
        api('/api/clients'),
        api('/api/tunnels'),
        api('/api/metrics'),
      ])
      set({
        clients,
        tunnels: tunnels.tunnels,
        metrics,
        lastSync: Date.now(),
        loading: false,
        error: null,
      })
    } catch (e) {
      set({ error: String(e?.message || e), loading: false })
    }
  },
}))