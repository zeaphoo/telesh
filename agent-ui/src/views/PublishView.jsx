import { useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowRightLeft,
  Globe,
  Loader2,
  Network,
  Plus,
  RotateCcw,
  Save,
  Trash2,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { EmptyState } from '../components/empty-state'
import { PageHeader } from '../components/page-header'
import { StatusBadge } from '../components/status-badge'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '../components/ui/alert-dialog'
import { Button } from '../components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '../components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '../components/ui/table'
import { Tabs, TabsList, TabsTrigger } from '../components/ui/tabs'
import { useAgent } from '../store/useAgent'

const KINDS = ['none', 'http', 'tcp', 'p2p', 'mirror']

const SELECT_CLS = 'h-8 rounded-md border border-input bg-background px-2 text-xs'
const INPUT_CLS = 'h-8 w-full rounded-md border border-input bg-background px-2 text-xs'

const EMPTY_CFG = {
  kind: 'none',
  sub_domain: '',
  remote_port: '',
  name: '',
  remote: '',
}

/**
 * 把条目规范成可比较的形状:只保留 UI 能编辑的字段,按 kind + 端口排序。
 * 服务端返回的 EntryStatus 会多出 state/detail,这里忽略。
 * 依据:`daemon::publish_status` 存的是 desired entry 的 clone,字段不会被改写。
 */
function canonicalize(entries) {
  return entries
    .filter((e) => e && e.kind)
    .map((e) => {
      const out = { kind: e.kind, local_port: Number(e.local_port) }
      if (e.kind === 'http' && e.sub_domain) out.sub_domain = String(e.sub_domain)
      if (e.kind === 'tcp' && e.remote_port != null && e.remote_port !== '') {
        out.remote_port = Number(e.remote_port)
      }
      if (e.kind === 'p2p' && e.name) out.name = String(e.name)
      if (e.kind === 'mirror' && e.remote) out.remote = String(e.remote)
      return out
    })
    .sort((a, b) => a.kind.localeCompare(b.kind) || a.local_port - b.local_port)
}

const signature = (entries) => JSON.stringify(canonicalize(entries))

function StatusCell({ entry }) {
  if (!entry) return <span className="text-xs text-muted-foreground">—</span>
  const detail =
    entry.detail == null
      ? ''
      : typeof entry.detail === 'string'
        ? entry.detail
        : JSON.stringify(entry.detail)
  return (
    <div className="flex min-w-0 flex-col items-start gap-0.5">
      <StatusBadge state={entry.state} />
      {detail && (
        <span className="max-w-52 truncate text-xs text-muted-foreground" title={detail}>
          {detail}
        </span>
      )}
    </div>
  )
}

/** Ports of this machine + remote-service mirroring. */
export default function PublishView() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const {
    ports,
    portsError,
    publish,
    publishError,
    publishLoaded,
    publishing,
    lastSavedAt,
    savePublish,
  } = useAgent()

  // `cfg` is keyed by local_port and holds the *desired* config being edited.
  const [cfg, setCfg] = useState({})
  const [mirrors, setMirrors] = useState([])
  const [mirrorForm, setMirrorForm] = useState({ remote: '', local_port: '' })
  const [mirrorWarn, setMirrorWarn] = useState(null)
  const [filter, setFilter] = useState('all')
  const [removeAt, setRemoveAt] = useState(null)
  const seeded = useRef(false)

  const seedFrom = (list) => {
    const next = {}
    const mirrorRows = []
    for (const e of list) {
      if (!e || e.kind == null || e.local_port == null) continue
      if (e.kind === 'mirror') {
        mirrorRows.push({ local_port: String(e.local_port), remote: e.remote ?? '' })
        continue
      }
      next[String(e.local_port)] = {
        kind: e.kind,
        sub_domain: e.sub_domain ?? '',
        remote_port: e.remote_port == null ? '' : String(e.remote_port),
        name: e.name ?? '',
        remote: e.remote ?? '',
      }
    }
    setCfg(next)
    setMirrors(mirrorRows)
  }

  // Seed the draft from the server's desired state exactly once, so the 2s
  // poll can refresh runtime status without clobbering in-progress edits.
  useEffect(() => {
    if (seeded.current || !publishLoaded) return
    seedFrom(publish)
    seeded.current = true
  }, [publishLoaded, publish])

  // Rows = ports found on this machine, plus ports the server still publishes
  // that are no longer listening (so we never silently drop them on save).
  const rows = useMemo(() => {
    const out = []
    const seen = new Set()
    for (const p of ports) {
      const port = Number(p?.port)
      if (!Number.isFinite(port)) continue
      out.push({
        port,
        pid: p.pid ?? null,
        process: p.process ?? null,
        http_hint: !!p.http_hint,
        probed: !!p.probed,
        server: p.server ?? null,
        present: true,
      })
      seen.add(String(port))
    }
    for (const e of publish) {
      if (!e || e.kind === 'mirror' || e.local_port == null) continue
      const port = Number(e.local_port)
      if (!Number.isFinite(port) || seen.has(String(port))) continue
      out.push({
        port,
        pid: null,
        process: null,
        http_hint: false,
        probed: false,
        server: null,
        present: false,
      })
      seen.add(String(port))
    }
    out.sort((a, b) => a.port - b.port)
    return out
  }, [ports, publish])

  const cfgFor = (port) => cfg[String(port)] || EMPTY_CFG

  const setCfgFor = (port, patch) =>
    setCfg((c) => ({
      ...c,
      [String(port)]: { ...EMPTY_CFG, ...(c[String(port)] || {}), ...patch },
    }))

  const isPublished = (port) => cfgFor(port).kind !== 'none'

  // Desired-state array sent to PUT /api/publish: input fields only.
  const desired = useMemo(() => {
    const out = []
    for (const r of rows) {
      const c = cfg[String(r.port)]
      if (!c || c.kind === 'none') continue
      const entry = { kind: c.kind, local_port: r.port }
      if (c.kind === 'http' && c.sub_domain?.trim()) entry.sub_domain = c.sub_domain.trim()
      if (c.kind === 'tcp' && String(c.remote_port ?? '').trim()) {
        const n = Number(c.remote_port)
        if (Number.isFinite(n)) entry.remote_port = n
      }
      if (c.kind === 'p2p' && c.name?.trim()) entry.name = c.name.trim()
      if (c.kind === 'mirror' && c.remote?.trim()) entry.remote = c.remote.trim()
      out.push(entry)
    }
    for (const m of mirrors) {
      const entry = { kind: 'mirror', local_port: Number(m.local_port) }
      if (m.remote?.trim()) entry.remote = m.remote.trim()
      out.push(entry)
    }
    return out
  }, [rows, cfg, mirrors])

  const dirty = publishLoaded && signature(desired) !== signature(publish)

  // Warn before leaving with unsaved changes.
  useEffect(() => {
    if (!dirty) return undefined
    const handler = (e) => {
      e.preventDefault()
      e.returnValue = ''
    }
    window.addEventListener('beforeunload', handler)
    return () => window.removeEventListener('beforeunload', handler)
  }, [dirty])

  const visibleRows = useMemo(() => {
    switch (filter) {
      case 'listening':
        return rows.filter((r) => r.present)
      case 'http':
        return rows.filter((r) => r.http_hint)
      case 'published':
        return rows.filter((r) => isPublished(r.port))
      default:
        return rows
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows, filter, cfg])

  const statusFor = (kind, port) =>
    publish.find((e) => e && e.kind === kind && Number(e.local_port) === Number(port)) || null
  const entriesForPort = (port) =>
    publish.filter((e) => e && Number(e.local_port) === Number(port))

  const undo = () => {
    seedFrom(publish)
    setMirrorForm({ remote: '', local_port: '' })
    setMirrorWarn(null)
  }

  const addMirror = () => {
    const remote = mirrorForm.remote.trim()
    const rawPort = mirrorForm.local_port.trim()
    const port = Number(rawPort)
    if (!remote) {
      setMirrorWarn(t('publish.mirrors.warnRemote'))
      return
    }
    if (!rawPort || !Number.isFinite(port) || port <= 0 || port > 65535) {
      setMirrorWarn(t('publish.mirrors.warnPort'))
      return
    }
    if (mirrors.some((m) => Number(m.local_port) === port)) {
      setMirrorWarn(t('publish.mirrors.warnDuplicate', { port }))
      return
    }
    setMirrors((m) => [...m, { local_port: String(port), remote }])
    setMirrorForm({ remote: '', local_port: '' })
    setMirrorWarn(null)
  }

  const renderParam = (port, c) => {
    switch (c.kind) {
      case 'http':
        return (
          <input
            className={INPUT_CLS}
            value={c.sub_domain}
            onChange={(e) => setCfgFor(port, { sub_domain: e.target.value })}
            placeholder={t('publish.ports.placeholders.subDomain')}
          />
        )
      case 'tcp':
        return (
          <input
            className={INPUT_CLS}
            type="number"
            min="1"
            max="65535"
            value={c.remote_port}
            onChange={(e) => setCfgFor(port, { remote_port: e.target.value })}
            placeholder={t('publish.ports.placeholders.remotePort')}
          />
        )
      case 'p2p':
        return (
          <input
            className={INPUT_CLS}
            value={c.name}
            onChange={(e) => setCfgFor(port, { name: e.target.value })}
            placeholder={t('publish.ports.placeholders.name')}
          />
        )
      case 'mirror':
        return (
          <input
            className={INPUT_CLS}
            value={c.remote}
            onChange={(e) => setCfgFor(port, { remote: e.target.value })}
            placeholder={t('publish.ports.placeholders.remote')}
          />
        )
      default:
        return <span className="text-xs text-muted-foreground">—</span>
    }
  }

  const FILTERS = ['all', 'listening', 'http', 'published']

  return (
    <div className="mx-auto max-w-5xl space-y-4 px-4 py-6 sm:px-6">
      <PageHeader title={t('nav.publish')} />

      {publishError && (
        <Card>
          <CardContent className="text-sm text-muted-foreground">
            {t('publish.ports.publishError', { error: publishError })}
          </CardContent>
        </Card>
      )}

      {/* 1. Local ports */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Network className="size-4" /> {t('publish.ports.title')}
          </CardTitle>
          <CardDescription>{t('publish.ports.description', { n: rows.length })}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3 p-0">
          <div className="px-6">
            <Tabs value={filter} onValueChange={setFilter}>
              <TabsList>
                {FILTERS.map((f) => (
                  <TabsTrigger key={f} value={f}>
                    {t(`publish.ports.filters.${f}`)}
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
          </div>
          {portsError && (
            <p className="px-6 text-xs text-muted-foreground">
              {t('publish.ports.portsError', { error: portsError })}
            </p>
          )}
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-24">{t('publish.ports.columns.port')}</TableHead>
                <TableHead className="w-32">{t('publish.ports.columns.process')}</TableHead>
                <TableHead className="w-24">{t('publish.ports.columns.pid')}</TableHead>
                <TableHead className="w-40">{t('publish.ports.columns.kind')}</TableHead>
                <TableHead className="w-56">{t('publish.ports.columns.param')}</TableHead>
                <TableHead>{t('publish.ports.columns.status')}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {visibleRows.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={6} className="p-0">
                    <EmptyState
                      icon={Network}
                      title={portsError ? t('publish.ports.unavailable') : t('publish.ports.empty')}
                    />
                  </TableCell>
                </TableRow>
              ) : (
                visibleRows.map((r) => {
                  const c = cfgFor(r.port)
                  const status = c.kind === 'none' ? null : statusFor(c.kind, r.port)
                  const existing = c.kind === 'none' ? entriesForPort(r.port) : []
                  return (
                    <TableRow key={r.port}>
                      <TableCell className="font-mono text-xs">
                        {r.port}
                        {r.http_hint && (
                          <span title={t('publish.ports.detectedHttp')}>
                            <Globe className="ml-1 inline size-3 text-success" />
                          </span>
                        )}
                      </TableCell>
                      <TableCell className="text-xs">{r.process || '—'}</TableCell>
                      <TableCell className="font-mono text-xs text-muted-foreground">
                        {r.pid ?? '—'}
                      </TableCell>
                      <TableCell>
                        <div className="flex flex-col items-start gap-1">
                          <select
                            className={SELECT_CLS}
                            value={c.kind}
                            onChange={(e) => setCfgFor(r.port, { kind: e.target.value })}
                          >
                            {KINDS.map((k) => (
                              <option key={k} value={k}>
                                {k === 'none' ? t('publish.ports.kindNone') : tc(`kind.${k}`)}
                              </option>
                            ))}
                          </select>
                          <div className="flex flex-wrap items-center gap-1">
                            {r.probed && r.http_hint ? (
                              <>
                                <StatusBadge
                                  state="live"
                                  label={t('publish.ports.detectedHttp')}
                                />
                                {r.server && (
                                  <span
                                    className="max-w-28 truncate text-xs text-muted-foreground"
                                    title={r.server}
                                  >
                                    {r.server}
                                  </span>
                                )}
                              </>
                            ) : (
                              r.http_hint &&
                              c.kind !== 'http' && (
                                <StatusBadge
                                  state="pending"
                                  label={t('publish.ports.suggestHttp')}
                                />
                              )
                            )}
                            {!r.present && (
                              <StatusBadge state="offline" label={tc('status.notListening')} />
                            )}
                          </div>
                        </div>
                      </TableCell>
                      <TableCell>{renderParam(r.port, c)}</TableCell>
                      <TableCell>
                        {status ? (
                          <StatusCell entry={status} />
                        ) : existing.length > 0 ? (
                          <div className="flex flex-col gap-1">
                            {existing.map((e, i) => (
                              <div key={i} className="flex items-center gap-1.5">
                                <StatusBadge
                                  state="pending"
                                  label={tc(`kind.${e.kind}`, { defaultValue: e.kind })}
                                />
                                <StatusCell entry={e} />
                              </div>
                            ))}
                          </div>
                        ) : (
                          <span className="text-xs text-muted-foreground">—</span>
                        )}
                      </TableCell>
                    </TableRow>
                  )
                })
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      {/* 2. Mirrored remote services */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <ArrowRightLeft className="size-4" /> {t('publish.mirrors.title')}
          </CardTitle>
          <CardDescription>{t('publish.mirrors.description')}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex flex-wrap items-center gap-2">
            <input
              className={`${INPUT_CLS} w-56`}
              value={mirrorForm.remote}
              onChange={(e) => setMirrorForm((f) => ({ ...f, remote: e.target.value }))}
              placeholder={t('publish.mirrors.remotePlaceholder')}
            />
            <input
              className={`${INPUT_CLS} w-36`}
              type="number"
              min="1"
              max="65535"
              value={mirrorForm.local_port}
              onChange={(e) => setMirrorForm((f) => ({ ...f, local_port: e.target.value }))}
              placeholder={t('publish.mirrors.localPortPlaceholder')}
            />
            <Button variant="outline" size="sm" onClick={addMirror} className="gap-1">
              <Plus className="size-3.5" /> {t('publish.mirrors.add')}
            </Button>
          </div>
          {mirrorWarn && <p className="text-xs text-destructive">{mirrorWarn}</p>}

          {mirrors.length === 0 ? (
            <EmptyState icon={ArrowRightLeft} description={t('publish.mirrors.empty')} />
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t('publish.mirrors.columns.remote')}</TableHead>
                  <TableHead className="w-32">
                    {t('publish.mirrors.columns.localPort')}
                  </TableHead>
                  <TableHead>{t('publish.mirrors.columns.status')}</TableHead>
                  <TableHead className="w-16" />
                </TableRow>
              </TableHeader>
              <TableBody>
                {mirrors.map((m, i) => (
                  <TableRow key={`${m.local_port}-${i}`}>
                    <TableCell className="font-mono text-xs">{m.remote || '—'}</TableCell>
                    <TableCell className="font-mono text-xs">{m.local_port}</TableCell>
                    <TableCell>
                      <StatusCell entry={statusFor('mirror', m.local_port)} />
                    </TableCell>
                    <TableCell>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="gap-1 text-muted-foreground"
                        onClick={() => setRemoveAt(i)}
                      >
                        <Trash2 className="size-3.5" /> {tc('action.remove')}
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      {/* Sticky save bar */}
      <div className="sticky bottom-0 z-30 rounded-lg border bg-background/95 p-3 shadow-sm backdrop-blur">
        <div className="flex flex-wrap items-center gap-3">
          <Button
            onClick={() => savePublish(desired)}
            disabled={publishing || !dirty}
            className="gap-1"
          >
            {publishing ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Save className="size-4" />
            )}
            {tc('action.saveApply')}
          </Button>
          {dirty && (
            <Button variant="ghost" size="sm" onClick={undo} className="gap-1">
              <RotateCcw className="size-3.5" /> {tc('action.undo')}
            </Button>
          )}
          <span className="text-xs text-muted-foreground">
            {t('publish.footer.pending', { n: desired.length })} ·{' '}
            {t('publish.footer.autoRefresh')}
          </span>
          {dirty ? (
            <StatusBadge tone="warning" label={t('publish.footer.unsaved')} />
          ) : (
            lastSavedAt &&
            !publishError && (
              <StatusBadge
                tone="success"
                label={`${tc('action.saved')} ${new Date(lastSavedAt).toLocaleTimeString()}`}
              />
            )
          )}
        </div>
      </div>

      <AlertDialog open={removeAt != null} onOpenChange={(open) => !open && setRemoveAt(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t('publish.mirrors.removeTitle')}</AlertDialogTitle>
            <AlertDialogDescription>
              {t('publish.mirrors.removeConfirm', {
                remote: removeAt != null ? mirrors[removeAt]?.remote || '' : '',
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{tc('action.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                setMirrors((list) => list.filter((_, j) => j !== removeAt))
                setRemoveAt(null)
              }}
            >
              {tc('action.remove')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
