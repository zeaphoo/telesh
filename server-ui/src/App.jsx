import { useEffect } from 'react'
import { Activity, RefreshCw, Server, Waypoints } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Route, Routes } from 'react-router-dom'

import { AppShell } from './components/app-shell'
import { LiveIndicator } from './components/live-indicator'
import { LocaleSwitcher } from './components/locale-switcher'
import { ThemeToggle } from './components/theme-toggle'
import { Button } from './components/ui/button'
import { Toaster } from './components/ui/sonner'
import Clients from './pages/Clients'
import Overview from './pages/Overview'
import Tunnels from './pages/Tunnels'
import { useDashboard } from './store/useDashboard'

const NAV = [
  { to: '/', labelKey: 'nav.overview', icon: Activity, end: true },
  { to: '/clients', labelKey: 'nav.clients', icon: Server },
  { to: '/tunnels', labelKey: 'nav.tunnels', icon: Waypoints },
]

export default function App() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const fetchAll = useDashboard((s) => s.fetchAll)
  const error = useDashboard((s) => s.error)
  const lastSync = useDashboard((s) => s.lastSync)

  // Auto-refresh every 3s while the page is open.
  useEffect(() => {
    fetchAll()
    const timer = setInterval(fetchAll, 3000)
    return () => clearInterval(timer)
  }, [fetchAll])

  const brand = (
    <span className="flex items-center gap-2 text-sm font-semibold tracking-tight">
      <img src="/icon.svg?v=3" alt="" className="size-5" /> {t('title')}
    </span>
  )

  const status = (
    <span className="hidden md:flex">
      <LiveIndicator online={!error} lastSync={lastSync} />
    </span>
  )

  const actions = (
    <>
      <Button
        variant="ghost"
        size="icon-sm"
        onClick={() => fetchAll()}
        aria-label={tc('action.refresh')}
      >
        <RefreshCw />
      </Button>
      <LocaleSwitcher />
      <ThemeToggle />
    </>
  )

  return (
    <AppShell brand={brand} status={status} actions={actions} nav={NAV}>
      <Routes>
        <Route path="/" element={<Overview />} />
        <Route path="/clients" element={<Clients />} />
        <Route path="/tunnels" element={<Tunnels />} />
      </Routes>
      <Toaster richColors position="top-right" />
    </AppShell>
  )
}
