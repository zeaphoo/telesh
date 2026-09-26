import { useMemo, useState } from 'react'
import {
  ArrowRightLeft,
  Check,
  Globe,
  Loader2,
  Plus,
  Radio,
  Trash2,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { EmptyState } from '../components/empty-state'
import { PageHeader } from '../components/page-header'
import { StatusBadge } from '../components/status-badge'
import { Badge } from '../components/ui/badge'
import { Button } from '../components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '../components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../components/ui/dialog'
import { Label } from '../components/ui/label'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '../components/ui/table'
import { Tabs, TabsList, TabsTrigger } from '../components/ui/tabs'
import { cn } from '../lib/utils'
import { useAgent } from '../store/useAgent'

const PUBLIC_MODES = ['http', 'tcp']
const INPUT_CLS = 'h-9 w-full rounded-md border border-input bg-background px-3 text-sm'

/** 服务端状态条目 -> 可提交的 desired 条目(去掉 state/detail)。 */
function toDesired(entries) {
  return entries
    .filter((e) => e && e.kind && e.local_port != null)
    .map((e) => {
      const out = { kind: e.kind, local_port: Number(e.local_port) }
      if (e.kind === 'http' && e.sub_domain) out.sub_domain = e.sub_domain
      if (e.kind === 'tcp' && e.remote_port != null && e.remote_port !== '') {
        out.remote_port = Number(e.remote_port)
      }
      if (e.kind === 'p2p' && e.name) out.name = e.name
      if (e.kind === 'mirror' && e.remote) out.remote = e.remote
      return out
    })
}

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

/** 可勾选的端口列表(对话框与 P2P 规则共用同一视觉)。 */
function PortRow({ port, process, httpHint, selected, onSelect, hint }) {
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={selected}
      className={cn(
        'flex w-full items-center gap-3 border-b px-3 py-2 text-left transition-colors last:border-b-0',
        selected ? 'bg-accent text-accent-foreground' : 'hover:bg-muted'
      )}
    >
      <span className="w-6 shrink-0">
        {selected && <Check className="size-4" />}
      </span>
      <span className="w-16 shrink-0 font-mono text-xs tabular-nums">{port}</span>
      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
        {process || '—'}
      </span>
      {httpHint && hint}
    </button>
  )
}

/** 端口与服务:对外发布 + 远程连接/P2P。所有操作即时生效。 */
export default function PublishView() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const {
    ports,
    portsError,
    publish,
    publishError,
    publishing,
    applyPublish,
    p2pPolicy,
    applyP2pPolicy,
  } = useAgent()

  const [dialogOpen, setDialogOpen] = useState(false)
  const [selPort, setSelPort] = useState(null)
  const [mode, setMode] = useState('http')
  const [subDomain, setSubDomain] = useState('')
  const [remotePort, setRemotePort] = useState('')

  const [mirrorForm, setMirrorForm] = useState({ remote: '', local_port: '' })
  const [mirrorWarn, setMirrorWarn] = useState(null)

  const publicEntries = useMemo(
    () => publish.filter((e) => e && (e.kind === 'http' || e.kind === 'tcp')),
    [publish]
  )
  const mirrors = useMemo(() => publish.filter((e) => e && e.kind === 'mirror'), [publish])

  const policyMode = p2pPolicy?.mode || 'selected'
  const policyPorts = p2pPolicy?.ports || []

  // 去重后的监听端口(用端口号做 key)。
  const listening = useMemo(() => {
    const byPort = new Map()
    for (const p of ports) {
      const port = Number(p?.port)
      if (!Number.isFinite(port) || byPort.has(port)) continue
      byPort.set(port, { port, process: p.process ?? null, http_hint: !!p.http_hint })
    }
    return [...byPort.values()].sort((a, b) => a.port - b.port)
  }, [ports])

  // 尚未对外发布的端口(发布对话框里可选)。
  const usedPorts = useMemo(
    () => new Set(publish.map((e) => Number(e.local_port))),
    [publish]
  )
  const available = useMemo(
    () => listening.filter((p) => !usedPorts.has(p.port)),
    [listening, usedPorts]
  )
  const selectedInListening = useMemo(
    () => new Set(listening.map((p) => p.port)),
    [listening]
  )

  const hasAnything =
    publicEntries.length > 0 ||
    mirrors.length > 0 ||
    policyMode !== 'selected' ||
    policyPorts.length > 0

  async function apply(next, okMsg) {
    const ok = await applyPublish(next)
    if (ok) toast.success(okMsg)
    else toast.error(useAgent.getState().publishError || tc('error.api'))
    return ok
  }

  async function applyPolicy(patch, okMsg) {
    const next = { mode: policyMode, ports: policyPorts, ...patch }
    const ok = await applyP2pPolicy(next)
    if (ok) toast.success(okMsg)
    else toast.error(useAgent.getState().publishError || tc('error.api'))
    return ok
  }

  function openDialog() {
    setSelPort(null)
    setMode('http')
    setSubDomain('')
    setRemotePort('')
    setDialogOpen(true)
  }

  async function submitPublish() {
    if (selPort == null) return
    const entry = { kind: mode, local_port: selPort }
    if (mode === 'http' && subDomain.trim()) entry.sub_domain = subDomain.trim()
    if (mode === 'tcp' && remotePort.trim()) {
      const n = Number(remotePort)
      if (Number.isFinite(n)) entry.remote_port = n
    }
    const next = [...toDesired(publish).filter((e) => e.local_port !== selPort), entry]
    if (await apply(next, t('publish.added'))) setDialogOpen(false)
  }

  async function unpublish(entry) {
    const next = toDesired(publish).filter(
      (e) => !(e.kind === entry.kind && e.local_port === Number(entry.local_port))
    )
    await apply(next, t('publish.removed'))
  }

  async function addMirror() {
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
    const next = [...toDesired(publish), { kind: 'mirror', local_port: port, remote }]
    if (await apply(next, t('publish.mirrors.added'))) {
      setMirrorForm({ remote: '', local_port: '' })
      setMirrorWarn(null)
    }
  }

  async function removeMirror(entry) {
    const next = toDesired(publish).filter(
      (e) => !(e.kind === 'mirror' && e.local_port === Number(entry.local_port))
    )
    await apply(next, t('publish.mirrors.removed'))
  }

  function targetOf(e) {
    if (e.kind === 'http') return e.sub_domain || t('publish.targetAuto')
    if (e.kind === 'tcp') {
      return e.remote_port != null ? String(e.remote_port) : t('publish.targetAuto')
    }
    return '—'
  }

  function togglePolicyPort(port) {
    const set = new Set(policyPorts)
    if (set.has(port)) set.delete(port)
    else set.add(port)
    applyPolicy({ ports: [...set].sort((a, b) => a - b) }, t('publish.p2p.saved'))
  }

  const canSubmit = selPort != null && !publishing

  return (
    <div className="mx-auto max-w-5xl space-y-4 px-4 py-6 sm:px-6">
      <PageHeader
        title={t('nav.publish')}
        actions={
          <Button onClick={openDialog} className="gap-1">
            <Plus className="size-4" /> {t('publish.add')}
          </Button>
        }
      />

      {/* 一眼可见的概览:没配置任何东西时整行不显示 */}
      {hasAnything && (
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline">
            {t('publish.summary.published', { n: publicEntries.length })}
          </Badge>
          <Badge variant="outline">{t('publish.summary.mirrors', { n: mirrors.length })}</Badge>
          <Badge variant="outline">
            {t('publish.summary.p2p', { mode: t(`publish.p2p.${policyMode}`) })}
          </Badge>
          {publishError && (
            <span className="text-xs text-destructive">
              {t('publish.publishError', { error: publishError })}
            </span>
          )}
        </div>
      )}

      {/* 区块一:对外发布 */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Globe className="size-4" /> {t('publish.publishTitle')}
          </CardTitle>
          <CardDescription>{t('publish.publishDesc')}</CardDescription>
        </CardHeader>
        <CardContent className="p-0">
          {publicEntries.length === 0 ? (
            <EmptyState
              icon={Globe}
              title={t('publish.none')}
              description={t('publish.noneHint')}
              action={
                <Button variant="outline" size="sm" onClick={openDialog} className="gap-1">
                  <Plus className="size-3.5" /> {t('publish.add')}
                </Button>
              }
            />
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-28">{t('publish.columns.port')}</TableHead>
                  <TableHead>{t('publish.columns.target')}</TableHead>
                  <TableHead>{t('publish.columns.status')}</TableHead>
                  <TableHead className="w-28 text-right">
                    {t('publish.columns.actions')}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {publicEntries.map((e, i) => (
                  <TableRow key={`${e.kind}-${e.local_port}-${i}`}>
                    <TableCell className="font-mono text-xs">{e.local_port}</TableCell>
                    <TableCell className="font-mono text-xs">{targetOf(e)}</TableCell>
                    <TableCell>
                      <StatusCell entry={e} />
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="gap-1 text-muted-foreground"
                        disabled={publishing}
                        onClick={() => unpublish(e)}
                      >
                        <Trash2 className="size-3.5" /> {t('publish.unpublish')}
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      {/* 区块二:远程连接与 P2P */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <ArrowRightLeft className="size-4" /> {t('publish.remoteTitle')}
          </CardTitle>
          <CardDescription>{t('publish.remoteDesc')}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-5">
          {/* 2a. 连接远程端口 */}
          <section className="space-y-3">
            <div>
              <h3 className="text-sm font-medium">{t('publish.mirrors.title')}</h3>
              <p className="text-xs text-muted-foreground">{t('publish.mirrors.desc')}</p>
            </div>
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
              <Button
                variant="outline"
                size="sm"
                onClick={addMirror}
                disabled={publishing}
                className="gap-1"
              >
                <Plus className="size-3.5" /> {t('publish.mirrors.add')}
              </Button>
            </div>
            {mirrorWarn && <p className="text-xs text-destructive">{mirrorWarn}</p>}

            {mirrors.length === 0 ? (
              <p className="text-xs text-muted-foreground">{t('publish.mirrors.empty')}</p>
            ) : (
              <div className="overflow-hidden rounded-md border">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>{t('publish.mirrors.columns.remote')}</TableHead>
                      <TableHead className="w-32">
                        {t('publish.mirrors.columns.localPort')}
                      </TableHead>
                      <TableHead>{t('publish.mirrors.columns.status')}</TableHead>
                      <TableHead className="w-24 text-right" />
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {mirrors.map((m, i) => (
                      <TableRow key={`${m.local_port}-${i}`}>
                        <TableCell className="font-mono text-xs">{m.remote || '—'}</TableCell>
                        <TableCell className="font-mono text-xs">{m.local_port}</TableCell>
                        <TableCell>
                          <StatusCell entry={m} />
                        </TableCell>
                        <TableCell className="text-right">
                          <Button
                            variant="ghost"
                            size="sm"
                            className="gap-1 text-muted-foreground"
                            disabled={publishing}
                            onClick={() => removeMirror(m)}
                          >
                            <Trash2 className="size-3.5" /> {tc('action.remove')}
                          </Button>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            )}
          </section>

          {/* 2b. P2P 入站规则 */}
          <section className="space-y-3 border-t pt-5">
            <div>
              <h3 className="flex items-center gap-2 text-sm font-medium">
                <Radio className="size-3.5" /> {t('publish.p2p.title')}
              </h3>
              <p className="text-xs text-muted-foreground">{t('publish.p2p.desc')}</p>
            </div>
            <Tabs
              value={policyMode}
              onValueChange={(v) => applyPolicy({ mode: v }, t('publish.p2p.saved'))}
            >
              <TabsList>
                <TabsTrigger value="all">{t('publish.p2p.all')}</TabsTrigger>
                <TabsTrigger value="selected">{t('publish.p2p.selected')}</TabsTrigger>
                <TabsTrigger value="none">{t('publish.p2p.none')}</TabsTrigger>
              </TabsList>
            </Tabs>

            {policyMode === 'all' && (
              <p className="text-xs text-muted-foreground">
                {t('publish.p2p.allHint', { n: listening.length })}
              </p>
            )}
            {policyMode === 'none' && (
              <p className="text-xs text-muted-foreground">{t('publish.p2p.noneHint')}</p>
            )}
            {policyMode === 'selected' && (
              <div className="space-y-2">
                <p className="text-xs text-muted-foreground">
                  {t('publish.p2p.selectedHint', { n: policyPorts.length })}
                </p>
                {listening.length === 0 ? (
                  <p className="text-xs text-muted-foreground">{t('publish.p2p.noPorts')}</p>
                ) : (
                  <div className="max-h-64 overflow-auto rounded-md border">
                    {listening.map((p) => (
                      <PortRow
                        key={p.port}
                        port={p.port}
                        process={p.process}
                        selected={policyPorts.includes(p.port)}
                        onSelect={() => togglePolicyPort(p.port)}
                        hint={
                          p.http_hint ? (
                            <StatusBadge state="live" label={t('publish.detectedHttp')} />
                          ) : null
                        }
                      />
                    ))}
                  </div>
                )}
                {policyPorts.some((p) => !selectedInListening.has(p)) && (
                  <p className="text-xs text-muted-foreground">
                    {t('publish.p2p.offlinePorts', {
                      ports: policyPorts.filter((p) => !selectedInListening.has(p)).join(', '),
                    })}
                  </p>
                )}
              </div>
            )}
          </section>
        </CardContent>
      </Card>

      {/* 发布对话框(HTTP / TCP) */}
      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{t('publish.dialog.title')}</DialogTitle>
            <DialogDescription>{t('publish.dialog.description')}</DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <div>
              <p className="mb-2 text-sm font-medium">{t('publish.dialog.ports')}</p>
              {portsError ? (
                <p className="text-xs text-muted-foreground">
                  {t('publish.dialog.portsError', { error: portsError })}
                </p>
              ) : available.length === 0 ? (
                <EmptyState icon={Globe} description={t('publish.dialog.noPorts')} />
              ) : (
                <div className="max-h-56 overflow-auto rounded-md border">
                  {available.map((p) => (
                    <PortRow
                      key={p.port}
                      port={p.port}
                      process={p.process}
                      selected={selPort === p.port}
                      onSelect={() => setSelPort(p.port)}
                      hint={
                        p.http_hint ? (
                          <StatusBadge state="live" label={t('publish.detectedHttp')} />
                        ) : null
                      }
                    />
                  ))}
                </div>
              )}
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label htmlFor="publish-mode">{t('publish.dialog.mode')}</Label>
                <select
                  id="publish-mode"
                  className={INPUT_CLS}
                  value={mode}
                  onChange={(e) => setMode(e.target.value)}
                >
                  {PUBLIC_MODES.map((m) => (
                    <option key={m} value={m}>
                      {tc(`kind.${m}`)}
                    </option>
                  ))}
                </select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="publish-param">{t('publish.dialog.params')}</Label>
                {mode === 'http' && (
                  <input
                    id="publish-param"
                    className={INPUT_CLS}
                    value={subDomain}
                    onChange={(e) => setSubDomain(e.target.value)}
                    placeholder={t('publish.dialog.placeholders.subDomain')}
                  />
                )}
                {mode === 'tcp' && (
                  <input
                    id="publish-param"
                    className={INPUT_CLS}
                    type="number"
                    min="1"
                    max="65535"
                    value={remotePort}
                    onChange={(e) => setRemotePort(e.target.value)}
                    placeholder={t('publish.dialog.placeholders.remotePort')}
                  />
                )}
              </div>
            </div>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setDialogOpen(false)}>
              {tc('action.cancel')}
            </Button>
            <Button onClick={submitPublish} disabled={!canSubmit} className="gap-1">
              {publishing && <Loader2 className="size-4 animate-spin" />}
              {t('publish.dialog.confirm')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
