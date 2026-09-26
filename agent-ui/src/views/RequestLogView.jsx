import { useEffect, useMemo, useRef, useState } from 'react'
import { Activity, Inbox } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { CopyButton } from '../components/copy-button'
import { EmptyState } from '../components/empty-state'
import { PageHeader } from '../components/page-header'
import { StatusBadge } from '../components/status-badge'
import { Button } from '../components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '../components/ui/card'
import { Label } from '../components/ui/label'
import { Skeleton } from '../components/ui/skeleton'
import { Switch } from '../components/ui/switch'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '../components/ui/table'
import { formatBytes } from '../lib/format'
import { useAgent } from '../store/useAgent'

function Payload({ title, text, emptyLabel, copyLabel }) {
  return (
    <div className="min-w-0 flex-1">
      <div className="mb-1 flex items-center gap-1">
        <p className="text-xs font-medium text-muted-foreground">{title}</p>
        {text && <CopyButton value={text} aria-label={copyLabel} />}
      </div>
      <pre className="max-h-72 overflow-auto rounded-md bg-muted p-3 text-xs whitespace-pre-wrap break-all">
        {text || emptyLabel}
      </pre>
    </div>
  )
}

/** HTTP status code -> 语义 tone。 */
function statusTone(code) {
  if (!code) return 'muted'
  if (code < 300) return 'success'
  if (code < 400) return 'info'
  if (code < 500) return 'warning'
  return 'danger'
}

function LogSkeleton() {
  return (
    <TableBody>
      {Array.from({ length: 6 }).map((_, i) => (
        <TableRow key={i}>
          <TableCell><Skeleton className="h-4 w-16" /></TableCell>
          <TableCell><Skeleton className="h-4 w-12" /></TableCell>
          <TableCell><Skeleton className="h-4 w-32" /></TableCell>
          <TableCell><Skeleton className="h-4 w-10" /></TableCell>
          <TableCell><Skeleton className="h-4 w-8" /></TableCell>
        </TableRow>
      ))}
    </TableBody>
  )
}

/** The original inspector view: rolling request log + detail/replay. */
export default function RequestLogView() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const {
    requests,
    loading,
    selected,
    replayOut,
    replaying,
    filters,
    setFilters,
    select,
    replay,
  } = useAgent()

  const [paused, setPaused] = useState(false)
  const [snapshot, setSnapshot] = useState([])
  const filterInputRef = useRef(null)

  // Pausing freezes the list for inspection; resuming returns to the live feed.
  const togglePause = () => {
    if (!paused) setSnapshot(requests)
    setPaused(!paused)
  }
  const source = paused ? snapshot : requests

  const methods = useMemo(
    () => [...new Set(source.map((r) => r.method).filter(Boolean))],
    [source]
  )
  const filtered = useMemo(() => {
    const { method, q } = filters
    return source.filter((r) => {
      if (method && r.method !== method) return false
      if (q) {
        const hay = `${r.path} ${r.status ?? ''} ${r.method}`.toLowerCase()
        if (!hay.includes(q.toLowerCase())) return false
      }
      return true
    })
  }, [source, filters])

  // Keyboard: j/k move selection, r replays, / focuses the filter.
  const filteredRef = useRef(filtered)
  filteredRef.current = filtered
  const selectedIdRef = useRef(selected?.id)
  selectedIdRef.current = selected?.id

  useEffect(() => {
    function onKey(e) {
      const el = e.target
      const isField =
        el && (el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA')
      if (isField) {
        if (e.key === 'Escape') el.blur()
        return
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return

      const list = filteredRef.current
      if (e.key === 'j' || e.key === 'k') {
        if (list.length === 0) return
        e.preventDefault()
        const idx = list.findIndex((r) => r.id === selectedIdRef.current)
        const nextIdx =
          e.key === 'j'
            ? Math.min(list.length - 1, idx < 0 ? 0 : idx + 1)
            : Math.max(0, idx < 0 ? list.length - 1 : idx - 1)
        select(list[nextIdx].id)
      } else if (e.key === 'r') {
        if (selectedIdRef.current) replay(selectedIdRef.current.id)
      } else if (e.key === '/') {
        e.preventDefault()
        filterInputRef.current?.focus()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [select, replay])

  const showSkeleton = loading && source.length === 0

  return (
    <div className="mx-auto max-w-5xl space-y-4 px-4 py-6 sm:px-6">
      <PageHeader title={t('nav.log')} description={t('log.shortcuts')} />

      <div className="grid gap-4 lg:grid-cols-[2fr_3fr]">
        {/* Request log */}
        <Card>
          <CardHeader className="flex-row items-center justify-between space-y-0 gap-3">
            <div>
              <CardTitle className="text-base">{t('log.listTitle')}</CardTitle>
              <CardDescription>
                {t('log.count', { shown: filtered.length, total: source.length })}
              </CardDescription>
            </div>
            <div className="flex items-center gap-2">
              <select
                value={filters.method}
                onChange={(e) => setFilters({ method: e.target.value })}
                className="h-8 rounded-md border border-input bg-background px-2 text-xs"
              >
                <option value="">{t('log.anyMethod')}</option>
                {methods.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
              <input
                ref={filterInputRef}
                value={filters.q}
                onChange={(e) => setFilters({ q: e.target.value })}
                placeholder={t('log.filterPlaceholder')}
                className="h-8 w-40 rounded-md border border-input bg-background px-2 text-xs"
              />
            </div>
          </CardHeader>
          <CardContent className="space-y-3 p-0">
            <div className="flex items-center gap-2 px-6">
              <Switch id="live-tail" checked={!paused} onCheckedChange={togglePause} />
              <Label htmlFor="live-tail" className="text-xs text-muted-foreground">
                {paused ? t('log.paused') : t('log.live')}
              </Label>
            </div>
            <div className="max-h-[70vh] overflow-auto">
              <Table>
                <TableHeader className="sticky top-0 bg-card">
                  <TableRow>
                    <TableHead>{t('log.columns.time')}</TableHead>
                    <TableHead>{t('log.columns.method')}</TableHead>
                    <TableHead>{t('log.columns.path')}</TableHead>
                    <TableHead>{t('log.columns.status')}</TableHead>
                    <TableHead>{t('log.columns.ms')}</TableHead>
                  </TableRow>
                </TableHeader>
                {showSkeleton ? (
                  <LogSkeleton />
                ) : (
                  <TableBody>
                    {filtered.length === 0 ? (
                      <TableRow>
                        <TableCell colSpan={5} className="p-0">
                          <EmptyState
                            icon={Inbox}
                            description={
                              source.length === 0 ? t('log.empty') : t('log.emptyFiltered')
                            }
                          />
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
                            <StatusBadge
                              tone={r.method === 'GET' ? 'muted' : 'info'}
                              label={r.method || '?'}
                            />
                          </TableCell>
                          <TableCell className="max-w-40 truncate font-mono text-xs">
                            {r.path || t('log.opaque')}
                          </TableCell>
                          <TableCell>
                            <StatusBadge tone={statusTone(r.status)} label={r.status ?? '—'} />
                          </TableCell>
                          <TableCell className="font-mono text-xs tabular-nums">
                            {r.duration_ms}
                          </TableCell>
                        </TableRow>
                      ))
                    )}
                  </TableBody>
                )}
              </Table>
            </div>
          </CardContent>
        </Card>

        {/* Detail + replay */}
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">{t('log.detail')}</CardTitle>
              <CardDescription>
                {selected
                  ? `${selected.method || '?'} ${selected.path || t('log.opaque')} · ${selected.duration_ms}ms · ${formatBytes(selected.req_bytes)} in / ${formatBytes(selected.resp_bytes)} out`
                  : t('log.detailHint')}
              </CardDescription>
            </CardHeader>
            <CardContent>
              {selected ? (
                <div className="flex flex-col gap-3 sm:flex-row">
                  <Payload
                    title={t('log.payloadRequest')}
                    text={selected.payload}
                    emptyLabel={t('log.emptyPayload')}
                    copyLabel={tc('action.copy')}
                  />
                  <Payload
                    title={t('log.payloadResponse')}
                    text={selected.resp_payload}
                    emptyLabel={t('log.emptyPayload')}
                    copyLabel={tc('action.copy')}
                  />
                </div>
              ) : (
                <EmptyState icon={Inbox} description={t('log.selectHint')} />
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">{t('log.replayTitle')}</CardTitle>
              <CardDescription>{t('log.replayDescription')}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <Button
                disabled={!selected || replaying}
                onClick={() => selected && replay(selected.id)}
                className="gap-1"
              >
                <Activity className="size-4" />
                {replaying ? t('log.replaying') : tc('action.replay')}
              </Button>
              {replayOut && (
                <pre className="max-h-48 overflow-auto rounded-md bg-muted p-3 text-xs whitespace-pre-wrap break-all">
                  {replayOut}
                </pre>
              )}
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  )
}
