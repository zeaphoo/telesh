import { useEffect, useMemo, useState } from 'react'
import { Activity, ArrowDownUp, Bug, Network, RefreshCw, Server } from 'lucide-react'
import { useTranslation } from 'react-i18next'

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
import PublishView from './views/PublishView'
import RequestLogView from './views/RequestLogView'

const NAV = [
  { id: 'publish', labelKey: 'nav.publish', icon: Network },
  { id: 'log', labelKey: 'nav.log', icon: Server },
]

export default function App() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const { tunnels, requests, error, lastSync, fetchAll, fetchPublish, refreshAll } = useAgent()
  const [tab, setTab] = useState('publish')

  // Poll every 2s while the page is open (requests + ports/publish status).
  useEffect(() => {
    fetchAll()
    fetchPublish()
    const timer = setInterval(() => {
      fetchAll()
      fetchPublish()
    }, 2000)
    return () => clearInterval(timer)
  }, [fetchAll, fetchPublish])

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
    <span className="flex min-w-0 items-center gap-2 text-sm font-semibold tracking-tight">
      <Bug className="size-4 shrink-0" />
      <span className="truncate">{t('title')}</span>
      {tunnel && (
        <span className="hidden min-w-0 items-center gap-1.5 text-xs font-normal text-muted-foreground md:flex">
          <Badge variant={tunnel.mode === 'tcp' ? 'secondary' : 'default'}>
            {tunnel.mode}
          </Badge>
          <span className="truncate font-mono">{tunnel.remote}</span>
          <span>→</span>
          <span className="truncate font-mono">{tunnel.local}</span>
        </span>
      )}
    </span>
  )

  const status = (
    <span className="hidden items-center gap-3 xl:flex">
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

      <footer className="mx-auto flex max-w-5xl items-center gap-2 px-4 pb-6 text-xs text-muted-foreground sm:px-6">
        <Activity className="size-3.5" />
        telesh agent · 127.0.0.1:16345
      </footer>

      <Toaster richColors position="top-right" />
    </AppShell>
  )
}
