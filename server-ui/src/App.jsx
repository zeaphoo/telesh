import { useEffect } from 'react'
import { NavLink, Route, Routes } from 'react-router-dom'

import { cn } from './lib/utils'
import Clients from './pages/Clients'
import Overview from './pages/Overview'
import Tunnels from './pages/Tunnels'
import { useDashboard } from './store/useDashboard'

const NAV = [
  { to: '/', label: 'Overview', end: true },
  { to: '/clients', label: 'Clients' },
  { to: '/tunnels', label: 'Tunnels' },
]

export default function App() {
  const fetchAll = useDashboard((s) => s.fetchAll)

  // Auto-refresh every 3s while the page is open.
  useEffect(() => {
    fetchAll()
    const t = setInterval(fetchAll, 3000)
    return () => clearInterval(t)
  }, [fetchAll])

  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="border-b bg-card">
        <div className="mx-auto flex h-14 max-w-6xl items-center gap-8 px-6">
          <span className="text-lg font-semibold tracking-tight">telesh</span>
          <nav className="flex gap-1">
            {NAV.map((n) => (
              <NavLink
                key={n.to}
                to={n.to}
                end={n.end}
                className={({ isActive }) =>
                  cn(
                    'rounded-md px-3 py-1.5 text-sm transition-colors',
                    isActive
                      ? 'bg-primary text-primary-foreground'
                      : 'hover:bg-muted'
                  )
                }
              >
                {n.label}
              </NavLink>
            ))}
          </nav>
        </div>
      </header>

      <main className="mx-auto max-w-6xl px-6 py-8">
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="/clients" element={<Clients />} />
          <Route path="/tunnels" element={<Tunnels />} />
        </Routes>
      </main>
    </div>
  )
}