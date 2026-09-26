import { useEffect, useMemo, useState } from 'react'
import { Activity, ArrowDownUp, Network, RefreshCw, Server, SquareTerminal, UserRound } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { AccountIndicator } from './components/account-indicator'
import { AppShell } from './components/app-shell'
import { LiveIndicator } from './components/live-indicator'
import { LocaleSwitcher } from './components/locale-switcher'
import { ThemeToggle } from './components/theme-toggle'
import { Badge } from './components/ui/badge'
import { Button } from './components/ui/button'
import { Card, CardContent } from './components/ui/card'
import { Toaster } from './components/ui/sonner'
import { formatBytes } from './lib/format'
import { useAgent } from './store/useAgent'
import AccountView from './views/AccountView'
import PublishView from './views/PublishView'
import RequestLogView from './views/RequestLogView'

const NAV = [
  { id: 'publish', labelKey: 'nav.publish', icon: Network },
  { id: 'log', labelKey: 'nav.log', icon: Server },
  { id: 'account', labelKey: 'nav.account', icon: UserRound },
]

export default function App() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const {
    tunnels,
    requests,
    account,
    error,
    lastSync,
    fetchAll,
    fetchPublish,
    fetchAccount,
    refreshAll,
  } = useAgent()
  const [tab, setTab] = useState('publish')

  // Poll every 2s while the page is open (requests + ports/publish status).
  useEffect(() => {
    fetchAll()
    fetchPublish()
    fetchAccount()
    const timer = setInterval(() => {
      fetchAll()
      fetchPublish()
      fetchAccount()
    }, 2000)
    return () => clearInterval(timer)
  }, [fetchAll, fetchPublish, fetchAccount])

  const tunnel = tunnels[0]
  const totals = useMemo(() => {
    let req = 0
    let resp = 0
    for (const r of requests) {
      req += r.req_bytes
      resp += r.resp_bytes
    }
    return { req, resp }
  }, [requests])

  const brand = (
    <span className="flex shrink-0 items-center gap-2 text-sm font-semibold tracking-tight">
      <SquareTerminal className="size-4" />
      {t('title')}
    </span>
  )

  const status = (
    <span className="hidden items-center gap-3 xl:flex">
      <AccountIndicator account={account} />
      {tunnel && (
        <span className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
          <Badge variant={tunnel.mode === 'tcp' ? 'secondary' : 'default'}>
            {tunnel.mode}
          </Badge>
          <span className="max-w-40 truncate font-mono">{tunnel.remote}</span>
          <span>→</span>
          <span className="max-w-40 truncate font-mono">{tunnel.local}</span>
        </span>
      )}
      <span className="flex items-center gap-1 text-xs text-muted-foreground">
        <Server className="size-3.5" /> {t('header.requests', { n: requests.length })}
      </span>
      <span className="flex items-center gap-1 text-xs text-muted-foreground">
        <ArrowDownUp className="size-3.5" /> ↑ {formatBytes(totals.req)} ↓{' '}
        {formatBytes(totals.resp)}
      </span>
      <LiveIndicator online={!error} lastSync={lastSync} />
    </span>
  )

  const actions = (
    <>
      <Button variant="ghost" size="sm" onClick={() => refreshAll()} className="gap-1">
        <RefreshCw className="size-3.5" />
        <span className="hidden sm:inline">{tc('action.refresh')}</span>
      </Button>
      <LocaleSwitcher />
      <ThemeToggle />
    </>
  )

  return (
    <AppShell
      brand={brand}
      status={status}
      actions={actions}
      nav={NAV}
      active={tab}
      onNavigate={(item) => setTab(item.id)}
    >
      {error && (
        <div className="px-4 pt-6 sm:px-6">
          <Card>
            <CardContent className="text-sm text-muted-foreground">
              {tc('error.api')}: {error}
            </CardContent>
          </Card>
        </div>
      )}

      {/* Both views stay mounted so unsaved port/publish drafts survive tab switches. */}
      <div className={tab === 'log' ? undefined : 'hidden'}>
        <RequestLogView />
      </div>
      <div className={tab === 'publish' ? undefined : 'hidden'}>
        <PublishView />
      </div>
      <div className={tab === 'account' ? undefined : 'hidden'}>
        <AccountView />
      </div>

      <footer className="mx-auto flex max-w-5xl items-center gap-2 px-4 pb-6 text-xs text-muted-foreground sm:px-6">
        <Activity className="size-3.5" />
        telesh agent · 127.0.0.1:16345
      </footer>

      <Toaster richColors position="top-right" />
    </AppShell>
  )
}
