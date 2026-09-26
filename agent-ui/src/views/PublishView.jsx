import { useMemo, useState } from 'react'
import { ArrowRightLeft, Loader2, Network, Plus, Trash2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

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
import { cn } from '../lib/utils'
import { useAgent } from '../store/useAgent'

const MODES = ['http', 'tcp', 'p2p']

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

/** 已发布的端口 + 即时发布/取消 + 远端服务镜像。 */
export default function PublishView() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const { ports, portsError, publish, publishError, publishing, applyPublish } = useAgent()

  const [dialogOpen, setDialogOpen] = useState(false)
  const [selPort, setSelPort] = useState(null)
  const [mode, setMode] = useState('http')
  const [subDomain, setSubDomain] = useState('')
  const [remotePort, setRemotePort] = useState('')
  const [name, setName] = useState('')

  const [mirrorForm, setMirrorForm] = useState({ remote: '', local_port: '' })
  const [mirrorWarn, setMirrorWarn] = useState(null)

  // 已发布的端口;镜像单独一节,不混进主表。
  const published = useMemo(
    () => publish.filter((e) => e && e.kind && e.kind !== 'mirror'),
    [publish]
  )
  const mirrors = useMemo(
    () => publish.filter((e) => e && e.kind === 'mirror'),
    [publish]
  )
  const publishedPorts = useMemo(
    () => new Set(published.map((e) => Number(e.local_port))),
    [published]
  )

  // 可选端口:按端口去重,并排除已经发布的。
  const available = useMemo(() => {
    const byPort = new Map()
    for (const p of ports) {
      const port = Number(p?.port)
      if (!Number.isFinite(port) || byPort.has(port)) continue
      byPort.set(port, {
        port,
        process: p.process ?? null,
        http_hint: !!p.http_hint,
      })
    }
    return [...byPort.values()]
      .filter((p) => !publishedPorts.has(p.port))
      .sort((a, b) => a.port - b.port)
  }, [ports, publishedPorts])

  /** 每次操作都把新的完整 desired 列表 PUT 上去,立即生效。 */
  async function apply(next, okMsg) {
    const ok = await applyPublish(next)
    if (ok) toast.success(okMsg)
    else toast.error(useAgent.getState().publishError || tc('error.api'))
    return ok
  }

  function openDialog() {
    setSelPort(null)
    setMode('http')
    setSubDomain('')
    setRemotePort('')
    setName('')
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
    if (mode === 'p2p' && name.trim()) entry.name = name.trim()

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
    if (e.kind === 'p2p') return e.name || '—'
    return e.remote || '—'
  }

  const canSubmit =
    selPort != null && !publishing && !(mode === 'p2p' && !name.trim())

  return (
    <div className="mx-auto max-w-5xl space-y-4 px-4 py-6 sm:px-6">
      <PageHeader
        title={t('nav.publish')}
        description={t('publish.description')}
        actions={
          <Button onClick={openDialog} className="gap-1">
            <Plus className="size-4" /> {t('publish.add')}
          </Button>
        }
      />

      {publishError && (
        <Card>
          <CardContent className="text-sm text-muted-foreground">
            {t('publish.publishError', { error: publishError })}
          </CardContent>
        </Card>
      )}

      {/* 1. 已发布的端口 */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Network className="size-4" /> {t('publish.title')}
          </CardTitle>
          <CardDescription>{t('publish.count', { n: published.length })}</CardDescription>
        </CardHeader>
        <CardContent className="p-0">
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
              {published.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={4} className="p-0">
                    <EmptyState
                      icon={Network}
                      description={t('publish.empty')}
                      action={
                        <Button variant="outline" size="sm" onClick={openDialog} className="gap-1">
                          <Plus className="size-3.5" /> {t('publish.add')}
                        </Button>
                      }
                    />
                  </TableCell>
                </TableRow>
              ) : (
                published.map((e, i) => (
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
                ))
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      {/* 2. 镜像远端服务 */}
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
          )}
        </CardContent>
      </Card>

      {/* 发布对话框:选本机端口 + 发布方式,确认后立即生效 */}
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
                <EmptyState icon={Network} description={t('publish.dialog.noPorts')} />
              ) : (
                <div className="max-h-56 overflow-auto rounded-md border">
                  {available.map((p) => (
                    <button
                      key={p.port}
                      type="button"
                      onClick={() => setSelPort(p.port)}
                      aria-pressed={selPort === p.port}
                      className={cn(
                        'flex w-full items-center gap-3 border-b px-3 py-2 text-left transition-colors last:border-b-0',
                        selPort === p.port
                          ? 'bg-accent text-accent-foreground'
                          : 'hover:bg-muted'
                      )}
                    >
                      <span className="w-16 font-mono text-xs">{p.port}</span>
                      <span className="flex-1 truncate text-xs text-muted-foreground">
                        {p.process || '—'}
                      </span>
                      {p.http_hint && (
                        <StatusBadge state="live" label={t('publish.detectedHttp')} />
                      )}
                    </button>
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
                  {MODES.map((m) => (
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
                {mode === 'p2p' && (
                  <input
                    id="publish-param"
                    className={INPUT_CLS}
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder={t('publish.dialog.placeholders.name')}
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
