import { useEffect, useMemo, useRef, useState } from 'react'
import { Inbox } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { CopyButton } from '../components/copy-button'
import { EmptyState } from '../components/empty-state'
import { StatusBadge } from '../components/status-badge'
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../components/ui/tabs'
import { formatBytes } from '../lib/format'
import { useAgent } from '../store/useAgent'

/** 原始 HTTP 报文 -> 起始行 / 头 / 体。 */
function splitMessage(raw) {
  if (!raw) return { startLine: '', headers: [], body: '' }
  let sep = raw.indexOf('\r\n\r\n')
  let sepLen = 4
  if (sep < 0) {
    sep = raw.indexOf('\n\n')
    sepLen = 2
  }
  const head = sep < 0 ? raw : raw.slice(0, sep)
  const body = sep < 0 ? '' : raw.slice(sep + sepLen)
  const lines = head.split(/\r?\n/)
  const startLine = lines.shift() || ''
  const headers = lines
    .map((line) => {
      const i = line.indexOf(':')
      return i < 0 ? null : [line.slice(0, i).trim(), line.slice(i + 1).trim()]
    })
    .filter(Boolean)
  return { startLine, headers, body }
}

/** 体是 JSON 就美化输出,否则原样。 */
function prettyBody(body) {
  const trimmed = body.trim()
  if (!trimmed) return ''
  try {
    return JSON.stringify(JSON.parse(trimmed), null, 2)
  } catch {
    return body
  }
}

function SectionLabel({ children, action }) {
  return (
    <div className="mb-1.5 flex items-center justify-between gap-2">
      <span className="text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
        {children}
      </span>
      {action}
    </div>
  )
}

/** 一段 HTTP 报文(请求或响应):起始行 + 头表 + 体。 */
function MessagePanel({ raw, emptyLabel }) {
  const { t } = useTranslation()
  const { startLine, headers, body } = useMemo(() => splitMessage(raw), [raw])
  const pretty = useMemo(() => prettyBody(body), [body])

  if (!raw) {
    return <p className="py-8 text-center text-xs text-muted-foreground">{emptyLabel}</p>
  }

  return (
    <div className="space-y-4">
      {startLine && (
        <div>
          <SectionLabel action={<CopyButton value={startLine} />}>
            {t('log.startLine')}
          </SectionLabel>
          <pre className="overflow-auto rounded-md bg-muted p-2.5 font-mono text-xs">
            {startLine}
          </pre>
        </div>
      )}

      <div>
        <SectionLabel
          action={
            headers.length > 0 && (
              <CopyButton value={headers.map(([k, v]) => `${k}: ${v}`).join('\n')} />
            )
          }
        >
          {t('log.headers')}
        </SectionLabel>
        {headers.length > 0 ? (
          <div className="overflow-hidden rounded-md border">
            <table className="w-full text-xs">
              <tbody>
                {headers.map(([k, v], i) => (
                  <tr key={`${k}-${i}`} className="border-b last:border-b-0">
                    <td className="w-48 max-w-48 border-r bg-muted/40 px-2.5 py-1 align-top font-mono font-medium text-muted-foreground">
                      {k}
                    </td>
                    <td className="min-w-0 px-2.5 py-1 align-top font-mono break-all">{v}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">{t('log.emptyPayload')}</p>
        )}
      </div>

      <div>
        <SectionLabel action={pretty && <CopyButton value={pretty} />}>
          {t('log.body')}
        </SectionLabel>
        {pretty ? (
          <pre className="overflow-auto rounded-md bg-muted p-2.5 font-mono text-xs whitespace-pre-wrap break-all">
            {pretty}
          </pre>
        ) : (
          <p className="text-xs text-muted-foreground">{t('log.emptyPayload')}</p>
        )}
      </div>
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
      {Array.from({ length: 8 }).map((_, i) => (
        <TableRow key={i}>
          <TableCell><Skeleton className="h-4 w-20" /></TableCell>
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

/** 请求追踪:左侧实时列表,右侧选中请求的详情(ngrok 式分栏)。 */
export default function RequestLogView() {
  const { t } = useTranslation()
  const {
    requests,
    loading,
    selected,
    filters,
    setFilters,
    select,
    channels,
    channel,
    setChannel,
    fetchAll,
  } = useAgent()

  const [paused, setPaused] = useState(false)
  const [snapshot, setSnapshot] = useState([])
  const filterInputRef = useRef(null)

  // 暂停冻结列表便于查看;恢复回到实时流。
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

  // 键盘:j/k 上下选择,`/` 聚焦过滤。
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
      } else if (e.key === '/') {
        e.preventDefault()
        filterInputRef.current?.focus()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [select])

  // 真实总数:选中通道取该通道,否则所有通道求和(接口只返回最近 100 条窗口)。
  const scopeTotal = useMemo(() => {
    if (channel) {
      return channels.find((c) => c.channel === channel)?.count ?? source.length
    }
    return channels.reduce((n, c) => n + c.count, 0)
  }, [channels, channel, source.length])

  const showSkeleton = loading && source.length === 0

  return (
    <div className="w-full px-4 py-4 sm:px-5">
      <div className="grid min-h-0 grid-cols-1 gap-3 xl:h-[calc(100vh-8rem)] xl:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)]">
        {/* 左:请求列表 */}
        <Card size="sm" className="min-h-0">
          <CardHeader className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex min-w-0 items-baseline gap-2">
              <CardTitle className="text-base">{t('log.listTitle')}</CardTitle>
              <CardDescription className="truncate">
                {t('log.count', { shown: filtered.length, total: scopeTotal })}
              </CardDescription>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <select
                value={channel}
                onChange={(e) => {
                  setChannel(e.target.value)
                  fetchAll()
                }}
                className="h-8 max-w-40 rounded-md border border-input bg-background px-2 text-xs"
              >
                <option value="">{t('log.allChannels')}</option>
                {channels.map((c) => (
                  <option key={c.channel} value={c.channel}>
                    {c.channel} ({c.count})
                  </option>
                ))}
              </select>
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
                className="h-8 w-36 rounded-md border border-input bg-background px-2 text-xs"
              />
            </div>
          </CardHeader>
          <CardContent className="flex min-h-0 flex-1 flex-col gap-2 p-0">
            <div className="flex items-center gap-2 px-3">
              <Switch id="live-tail" checked={!paused} onCheckedChange={togglePause} />
              <Label htmlFor="live-tail" className="text-xs text-muted-foreground">
                {paused ? t('log.paused') : t('log.live')}
              </Label>
            </div>
            <div className="max-h-[65vh] min-h-0 flex-1 overflow-auto xl:max-h-none">
              <Table>
                <TableHeader className="sticky top-0 z-10 bg-card">
                  <TableRow>
                    <TableHead>{t('log.columns.channel')}</TableHead>
                    <TableHead>{t('log.columns.time')}</TableHead>
                    <TableHead>{t('log.columns.method')}</TableHead>
                    <TableHead>{t('log.columns.path')}</TableHead>
                    <TableHead>{t('log.columns.status')}</TableHead>
                    <TableHead className="text-right">{t('log.columns.ms')}</TableHead>
                  </TableRow>
                </TableHeader>
                {showSkeleton ? (
                  <LogSkeleton />
                ) : (
                  <TableBody>
                    {filtered.length === 0 ? (
                      <TableRow>
                        <TableCell colSpan={6} className="p-0">
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
                          <TableCell className="max-w-28 truncate font-mono text-xs text-muted-foreground">
                            {r.channel}
                          </TableCell>
                          <TableCell className="whitespace-nowrap font-mono text-xs text-muted-foreground">
                            {r.started_at}
                          </TableCell>
                          <TableCell className="font-mono text-xs">
                            <StatusBadge
                              tone={r.method === 'GET' ? 'muted' : 'info'}
                              label={r.method || '?'}
                            />
                          </TableCell>
                          <TableCell className="max-w-96 truncate font-mono text-xs">
                            {r.path || t('log.opaque')}
                          </TableCell>
                          <TableCell>
                            <StatusBadge tone={statusTone(r.status)} label={r.status ?? '—'} />
                          </TableCell>
                          <TableCell className="text-right font-mono text-xs tabular-nums">
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

        {/* 右:请求详情 */}
        <Card size="sm" className="min-h-0">
          <CardHeader className="gap-1.5">
            <div className="flex min-w-0 items-center gap-2">
              <CardTitle className="shrink-0 text-base">{t('log.detail')}</CardTitle>
              {selected && (
                <>
                  <StatusBadge
                    tone={selected.method === 'GET' ? 'muted' : 'info'}
                    label={selected.method || '?'}
                  />
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">
                    {selected.path || t('log.opaque')}
                  </span>
                  <StatusBadge tone={statusTone(selected.status)} label={selected.status ?? '—'} />
                </>
              )}
            </div>
            <CardDescription className="truncate font-mono text-xs">
              {selected
                ? `${selected.channel} · ${selected.started_at} · ${selected.duration_ms}ms · ${formatBytes(selected.req_bytes)} in / ${formatBytes(selected.resp_bytes)} out`
                : t('log.detailHint')}
            </CardDescription>
          </CardHeader>
          <CardContent className="max-h-[80vh] min-h-0 flex-1 overflow-auto xl:max-h-none">
            {selected ? (
              <Tabs defaultValue="request">
                <TabsList>
                  <TabsTrigger value="request">{t('log.payloadRequest')}</TabsTrigger>
                  <TabsTrigger value="response">{t('log.payloadResponse')}</TabsTrigger>
                </TabsList>
                <TabsContent value="request" className="mt-3">
                  <MessagePanel raw={selected.payload} emptyLabel={t('log.emptyPayload')} />
                </TabsContent>
                <TabsContent value="response" className="mt-3">
                  <MessagePanel raw={selected.resp_payload} emptyLabel={t('log.emptyPayload')} />
                </TabsContent>
              </Tabs>
            ) : (
              <EmptyState icon={Inbox} description={t('log.selectHint')} />
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
