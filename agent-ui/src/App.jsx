import { useEffect, useMemo, useState } from 'react'
import { Activity, ArrowDownUp, Bug, Network, RefreshCw, Server } from 'lucide-react'

import { Badge } from './components/ui/badge'
import { Button } from './components/ui/button'
import {
  Card,
  CardContent,
} from './components/ui/card'
import { cn, formatBytes } from './lib/utils'
import { useAgent } from './store/useAgent'
import PublishView from './views/PublishView'
import RequestLogView from './views/RequestLogView'

const TABS = [
  { id: 'publish', label: '端口与发布', icon: Network },
  { id: 'log', label: '请求日志', icon: Server },
]

export default function App() {
  const { tunnels, requests, error, fetchAll, fetchPublish, refreshAll } = useAgent()
  const [tab, setTab] = useState('publish')

  // Poll every 2s while the page is open (requests + ports/publish status).
  useEffect(() => {
    fetchAll()
    fetchPublish()
    const t = setInterval(() => {
      fetchAll()
      fetchPublish()
    }, 2000)
    return () => clearInterval(t)
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

  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="border-b bg-card">
        <div className="mx-auto flex h-14 max-w-6xl items-center gap-4 px-6">
          <span className="flex items-center gap-2 text-lg font-semibold tracking-tight">
            <Bug className="size-5" /> telesh inspector
          </span>
          {tunnel && (
            <div className="flex items-center gap-2 text-sm">
              <Badge variant={tunnel.mode === 'tcp' ? 'secondary' : 'success'}>
                {tunnel.mode}
              </Badge>
              <span className="font-mono text-xs text-muted-foreground">{tunnel.remote}</span>
              <span className="text-xs text-muted-foreground">→</span>
              <span className="font-mono text-xs">{tunnel.local}</span>
            </div>
          )}
          <div className="ml-auto flex items-center gap-3 text-xs text-muted-foreground">
            <span className="flex items-center gap-1">
              <Server className="size-3.5" /> {requests.length} requests
            </span>
            <span className="flex items-center gap-1">
              <ArrowDownUp className="size-3.5" /> ↑ {formatBytes(totals.req)} ↓{' '}
              {formatBytes(totals.resp)}
            </span>
            <Button variant="ghost" size="sm" onClick={() => refreshAll()} className="gap-1">
              <RefreshCw className="size-3.5" /> refresh
            </Button>
          </div>
        </div>

        <div className="mx-auto flex max-w-6xl items-center gap-1 px-6 pb-2">
          {TABS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              onClick={() => setTab(id)}
              className={cn(
                'inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium transition-colors',
                tab === id
                  ? 'bg-primary text-primary-foreground'
                  : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground'
              )}
            >
              <Icon className="size-4" /> {label}
            </button>
          ))}
        </div>
      </header>

      {error && (
        <div className="mx-auto max-w-6xl px-6 pt-4">
          <Card>
            <CardContent className="py-4 text-sm text-muted-foreground">
              Cannot reach the inspector API: {error}
            </CardContent>
          </Card>
        </div>
      )}

      {/* Both views stay mounted so unsaved port/publish drafts survive tab switches. */}
      <main className={tab === 'log' ? undefined : 'hidden'}>
        <RequestLogView />
      </main>
      <main className={tab === 'publish' ? undefined : 'hidden'}>
        <PublishView />
      </main>

      <footer className="mx-auto flex max-w-6xl items-center gap-2 px-6 pb-6 text-xs text-muted-foreground">
        <Activity className="size-3.5" />
        telesh agent · 127.0.0.1:16345
      </footer>
    </div>
  )
}
