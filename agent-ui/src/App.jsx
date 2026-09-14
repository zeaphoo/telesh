import { useEffect, useMemo } from 'react'
import { Activity, ArrowDownUp, Bug, RefreshCw, Server } from 'lucide-react'

import { Badge } from './components/ui/badge'
import { Button } from './components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from './components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from './components/ui/table'
import { formatBytes, formatDuration } from './lib/utils'
import { useAgent } from './store/useAgent'

function Payload({ title, text }) {
  return (
    <div className="min-w-0 flex-1">
      <p className="mb-1 text-xs font-medium text-muted-foreground">{title}</p>
      <pre className="max-h-72 overflow-auto rounded-md bg-muted p-3 text-xs whitespace-pre-wrap break-all">
        {text || '(empty)'}
      </pre>
    </div>
  )
}

function statusVariant(status) {
  if (!status) return 'secondary'
  if (status < 300) return 'success'
  if (status < 500) return 'outline'
  return 'destructive'
}

export default function App() {
  const {
    tunnels,
    requests,
    selected,
    replayOut,
    replaying,
    error,
    fetchAll,
    select,
    replay,
  } = useAgent()

  // Poll every 2s while the page is open.
  useEffect(() => {
    fetchAll()
    const t = setInterval(fetchAll, 2000)
    return () => clearInterval(t)
  }, [fetchAll])

  const tunnel = tunnels[0]
  const { filters, setFilters } = useAgent()

  // Filtered + derived data.
  const methods = useMemo(() => [...new Set(requests.map((r) => r.method).filter(Boolean))], [requests])
  const filtered = useMemo(() => {
    const { method, q } = filters
    return requests.filter((r) => {
      if (method && r.method !== method) return false
      if (q) {
        const hay = `${r.path} ${r.status ?? ''} ${r.method}`.toLowerCase()
        if (!hay.includes(q.toLowerCase())) return false
      }
      return true
    })
  }, [requests, filters])
  const totals = useMemo(() => {
    let req = 0
    let resp = 0
    for (const r of filtered) {
      req += r.req_bytes
      resp += r.resp_bytes
    }
    return { req, resp }
  }, [filtered])

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
              <ArrowDownUp className="size-3.5" /> ↑ {formatBytes(totals.req)} ↓ {formatBytes(totals.resp)}
            </span>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => fetchAll()}
              className="gap-1"
            >
              <RefreshCw className="size-3.5" /> refresh
            </Button>
          </div>
        </div>
      </header>

      <main className="mx-auto grid max-w-6xl gap-4 px-6 py-6 lg:grid-cols-[2fr_3fr]">
        {error && (
          <Card className="lg:col-span-2">
            <CardContent className="py-4 text-sm text-muted-foreground">
              Cannot reach the inspector API: {error}
            </CardContent>
          </Card>
        )}

        {/* Request log */}
        <Card>
          <CardHeader className="flex-row items-center justify-between space-y-0 gap-3">
            <div>
              <CardTitle className="text-base">Requests</CardTitle>
              <CardDescription>
                {filtered.length} / {requests.length} shown · latest 100
              </CardDescription>
            </div>
            <div className="flex items-center gap-2">
              <select
                value={filters.method}
                onChange={(e) => setFilters({ method: e.target.value })}
                className="h-8 rounded-md border border-input bg-background px-2 text-xs"
              >
                <option value="">any method</option>
                {methods.map((m) => (
                  <option key={m} value={m}>{m}</option>
                ))}
              </select>
              <input
                value={filters.q}
                onChange={(e) => setFilters({ q: e.target.value })}
                placeholder="filter path/status…"
                className="h-8 w-40 rounded-md border border-input bg-background px-2 text-xs"
              />
            </div>
          </CardHeader>
          <CardContent className="p-0">
            <div className="max-h-[70vh] overflow-auto">
              <Table>
                <TableHeader className="sticky top-0 bg-card">
                  <TableRow>
                    <TableHead>time</TableHead>
                    <TableHead>method</TableHead>
                    <TableHead>path</TableHead>
                    <TableHead>status</TableHead>
                    <TableHead>ms</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {filtered.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">
                        {requests.length === 0
                          ? 'No requests yet — hit your tunnel URL to see requests here.'
                          : 'No requests match the filter.'}
                      </TableCell>
                    </TableRow>
                  ) : (
                    filtered.map((r) => (
                      <TableRow
                        key={r.id}
                        className="cursor-pointer"
                        data-state={selected?.id === r.id ? 'selected' : undefined}
                        onClick={() => select(r.id)}
                      >
                        <TableCell className="whitespace-nowrap font-mono text-xs text-muted-foreground">
                          {r.started_at}
                        </TableCell>
                        <TableCell className="font-mono text-xs">
                          <Badge variant={r.method === 'GET' ? 'secondary' : 'default'}>
                            {r.method || '?'}
                          </Badge>
                        </TableCell>
                        <TableCell className="max-w-40 truncate font-mono text-xs">
                          {r.path || '(binary/opaque)'}
                        </TableCell>
                        <TableCell>
                          <Badge variant={statusVariant(r.status)}>{r.status ?? '—'}</Badge>
                        </TableCell>
                        <TableCell className="font-mono text-xs">{r.duration_ms}</TableCell>
                      </TableRow>
                    ))
                  )}
                </TableBody>
              </Table>
            </div>
          </CardContent>
        </Card>

        {/* Detail + replay */}
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">Request detail</CardTitle>
              <CardDescription>
                {selected
                  ? `${selected.method || '?'} ${selected.path || '(opaque)'} · ${selected.duration_ms}ms · ${formatBytes(selected.req_bytes)} in / ${formatBytes(selected.resp_bytes)} out`
                  : 'Select a request on the left'}
              </CardDescription>
            </CardHeader>
            <CardContent>
              {selected ? (
                <div className="flex flex-col gap-3 sm:flex-row">
                  <Payload title="Request" text={selected.payload} />
                  <Payload title="Response" text={selected.resp_payload} />
                </div>
              ) : (
                <p className="py-6 text-center text-sm text-muted-foreground">
                  Click a request row to inspect its headers &amp; body.
                </p>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">Replay</CardTitle>
              <CardDescription>
                Re-send the captured request straight to the local service (dev aid)
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <Button
                disabled={!selected || replaying}
                onClick={() => selected && replay(selected.id)}
                className="gap-1"
              >
                <Activity className="size-4" />
                {replaying ? 'Replaying…' : 'Replay request'}
              </Button>
              {replayOut && (
                <pre className="max-h-48 overflow-auto rounded-md bg-muted p-3 text-xs whitespace-pre-wrap break-all">
                  {replayOut}
                </pre>
              )}
            </CardContent>
          </Card>
        </div>
      </main>
    </div>
  )
}