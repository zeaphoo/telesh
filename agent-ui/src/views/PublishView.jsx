import { useMemo, useState } from 'react'
import {
  ArrowRightLeft,
  Check,
  Globe,
  Loader2,
  Plus,
  Radio,
  Search,
  Settings2,
  Trash2,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { EmptyState } from '../components/empty-state'
import { StatusBadge } from '../components/status-badge'
import { Badge } from '../components/ui/badge'
import { Button } from '../components/ui/button'
import {
  Card,
  CardAction,
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
import { Switch } from '../components/ui/switch'
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

/** 可勾选的端口行(发布对话框与远程连接规则对话框共用)。 */
function PortRow({ port, process, hint, selected, onSelect }) {
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
      <span className="flex w-5 shrink-0 justify-center">
        {selected && <Check className="size-4" />}
      </span>
      <span className="w-16 shrink-0 font-mono text-xs tabular-nums">{port}</span>
      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
        {process || '—'}
      </span>
      {hint}
    </button>
  )
}

/** 发布和连接:对外发布 + 远程连接。所有操作即时生效。 */
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
    listServices,
    settings,
    applySettings,
  } = useAgent()

  // 发布对话框
  const [publishOpen, setPublishOpen] = useState(false)
  const [selPort, setSelPort] = useState(null)
  const [mode, setMode] = useState('http')
  const [subDomain, setSubDomain] = useState('')
  const [remotePort, setRemotePort] = useState('')

  // 连接远程端口对话框:先输 ACCOUNT 查询,再选服务 + 本地端口
  const [mirrorOpen, setMirrorOpen] = useState(false)
  const [account, setAccount] = useState('')
  const [services, setServices] = useState(null)
  const [querying, setQuerying] = useState(false)
  const [selService, setSelService] = useState(null)
  const [localPort, setLocalPort] = useState('')
  const [mirrorWarn, setMirrorWarn] = useState(null)

  // 远程连接规则对话框(草稿,点“保存”才提交)
  const [ruleOpen, setRuleOpen] = useState(false)
  const [ruleMode, setRuleMode] = useState('selected')
  const [rulePorts, setRulePorts] = useState([])

  const publicEntries = useMemo(
    () => publish.filter((e) => e && (e.kind === 'http' || e.kind === 'tcp')),
    [publish]
  )
  const mirrors = useMemo(() => publish.filter((e) => e && e.kind === 'mirror'), [publish])

  const policyMode = p2pPolicy?.mode || 'selected'
  const policyPorts = p2pPolicy?.ports || []

  // 请求追踪:按 (section, 端口) 存,publish 与 p2p 分开;没记录 = 默认开。
  function traceOn(section, port) {
    return settings?.[section]?.[String(port)]?.trace ?? true
  }
  function toggleTrace(section, port) {
    const key = String(port)
    const next = {
      publish: { ...(settings?.publish ?? {}) },
      p2p: { ...(settings?.p2p ?? {}) },
    }
    next[section][key] = { ...(next[section][key] ?? {}), trace: !traceOn(section, port) }
    applySettings(next)
  }

  // 去重后的监听端口(用端口号做 key)。
  const listening = useMemo(() => {
    const byPort = new Map()
    for (const p of ports) {
      const port = Number(p?.port)
      if (!Number.isFinite(port) || byPort.has(port)) continue
      byPort.set(port, {
        port,
        process: p.process ?? null,
        http_hint: !!p.http_hint,
        ssh_hint: !!p.ssh_hint,
      })
    }
    return [...byPort.values()].sort((a, b) => a.port - b.port)
  }, [ports])

  // 发布对话框只排除「已按当前方式发布」的端口:同一个本地端口允许同时发布为
  // HTTP 和 TCP,也允许在 P2P/镜像之外再发布,所以只按 kind 去重。
  const modePorts = useMemo(
    () => new Set(publish.filter((e) => e.kind === mode).map((e) => Number(e.local_port))),
    [publish, mode]
  )
  const available = useMemo(
    () => listening.filter((p) => !modePorts.has(p.port)),
    [listening, modePorts]
  )
  const listeningSet = useMemo(() => new Set(listening.map((p) => p.port)), [listening])
  const listeningByPort = useMemo(() => new Map(listening.map((p) => [p.port, p])), [listening])

  const protocolHint = (p) => {
    if (p.ssh_hint) return <StatusBadge state="live" label={t('publish.detectedSsh')} />
    if (p.http_hint) return <StatusBadge state="live" label={t('publish.detectedHttp')} />
    return null
  }

  async function apply(next, okMsg) {
    const ok = await applyPublish(next)
    if (ok) toast.success(okMsg)
    else toast.error(useAgent.getState().publishError || tc('error.api'))
    return ok
  }

  // ---- 发布 ----
  function openPublish() {
    setSelPort(null)
    setMode('http')
    setSubDomain('')
    setRemotePort('')
    setPublishOpen(true)
  }

  /** 选端口;SSH 不是 HTTP,默认切到 TCP。 */
  function pickPublishPort(p) {
    setSelPort(p.port)
    if (p.ssh_hint) setMode('tcp')
  }

  async function submitPublish() {
    if (selPort == null) return
    const entry = { kind: mode, local_port: selPort }
    if (mode === 'http' && subDomain.trim()) entry.sub_domain = subDomain.trim()
    if (mode === 'tcp' && remotePort.trim()) {
      const n = Number(remotePort)
      if (Number.isFinite(n)) entry.remote_port = n
    }
    const next = [
      ...toDesired(publish).filter((e) => !(e.kind === mode && e.local_port === selPort)),
      entry,
    ]
    if (await apply(next, t('publish.added'))) setPublishOpen(false)
  }

  async function unpublish(entry) {
    const next = toDesired(publish).filter(
      (e) => !(e.kind === entry.kind && e.local_port === Number(entry.local_port))
    )
    await apply(next, t('publish.removed'))
  }

  // ---- 连接远程端口 ----
  function openMirror() {
    setAccount('')
    setServices(null)
    setQuerying(false)
    setSelService(null)
    setLocalPort('')
    setMirrorWarn(null)
    setMirrorOpen(true)
  }

  async function queryServices() {
    const acct = account.trim()
    if (!acct) {
      setMirrorWarn(t('publish.mirrors.warnAccount'))
      return
    }
    setQuerying(true)
    setMirrorWarn(null)
    try {
      setServices(await listServices(acct))
      setSelService(null)
    } catch (e) {
      setServices([])
      setMirrorWarn(t('publish.mirrors.queryError', { error: e?.message || e }))
    } finally {
      setQuerying(false)
    }
  }

  function pickService(name) {
    setSelService(name)
    setMirrorWarn(null)
    // 服务名通常就是端口号:本地端口空着时顺手填上
    if (!localPort.trim() && /^\d+$/.test(name)) setLocalPort(name)
  }

  async function submitMirror() {
    const acct = account.trim()
    if (!acct || !selService) {
      setMirrorWarn(t('publish.mirrors.warnService'))
      return
    }
    const port = Number(localPort.trim())
    if (!localPort.trim() || !Number.isFinite(port) || port <= 0 || port > 65535) {
      setMirrorWarn(t('publish.mirrors.warnPort'))
      return
    }
    if (mirrors.some((m) => Number(m.local_port) === port)) {
      setMirrorWarn(t('publish.mirrors.warnDuplicate', { port }))
      return
    }
    const remote = `${acct}/${selService}`
    const next = [...toDesired(publish), { kind: 'mirror', local_port: port, remote }]
    if (await apply(next, t('publish.mirrors.added'))) setMirrorOpen(false)
  }

  async function removeMirror(entry) {
    const next = toDesired(publish).filter(
      (e) => !(e.kind === 'mirror' && e.local_port === Number(entry.local_port))
    )
    await apply(next, t('publish.mirrors.removed'))
  }

  // ---- 远程连接规则 ----
  function openRule() {
    setRuleMode(policyMode)
    setRulePorts(policyPorts)
    setRuleOpen(true)
  }

  function toggleRulePort(port) {
    setRulePorts((list) =>
      list.includes(port) ? list.filter((p) => p !== port) : [...list, port].sort((a, b) => a - b)
    )
  }

  async function submitRule() {
    const ok = await applyP2pPolicy({ mode: ruleMode, ports: rulePorts })
    const { publishError: err } = useAgent.getState()
    if (ok) {
      toast.success(t('publish.inbound.saved'))
      setRuleOpen(false)
    } else {
      toast.error(err || tc('error.api'))
    }
  }

  const modeSummary =
    policyMode === 'all'
      ? t('publish.inbound.allSummary', { n: listening.length })
      : policyMode === 'none'
        ? t('publish.inbound.noneSummary')
        : t('publish.inbound.selectedSummary', { n: policyPorts.length })

  // 直接把端口列出来(不只给个数字)。
  const reachable = useMemo(
    () =>
      policyMode === 'all'
        ? listening.map((p) => p.port)
        : policyMode === 'selected'
          ? policyPorts
          : [],
    [policyMode, listening, policyPorts]
  )

  function targetOf(e) {
    if (e.kind === 'http') return e.sub_domain || t('publish.targetAuto')
    if (e.kind === 'tcp') {
      return e.remote_port != null ? String(e.remote_port) : t('publish.targetAuto')
    }
    return '—'
  }

  return (
    <div className="mx-auto w-full max-w-5xl space-y-3 px-4 py-4 sm:px-5">
      {publishError && (
        <p className="text-xs text-destructive">
          {t('publish.publishError', { error: publishError })}
        </p>
      )}

      {/* 区块一:对外发布 */}
      <Card size="sm">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <Globe className="size-4" /> {t('publish.publishTitle')}
          </CardTitle>
          <CardDescription>{t('publish.publishDesc')}</CardDescription>
          <CardAction>
            <div className="flex items-center gap-2">
              <Badge variant="outline">
                {t('publish.summary.published', { n: publicEntries.length })}
              </Badge>
              <Button variant="outline" size="sm" onClick={openPublish} className="gap-1">
                <Plus className="size-3.5" /> {t('publish.add')}
              </Button>
            </div>
          </CardAction>
        </CardHeader>
        <CardContent className="p-0">
          {publicEntries.length === 0 ? (
            <EmptyState icon={Globe} title={t('publish.none')} description={t('publish.noneHint')} />
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-28">{t('publish.columns.port')}</TableHead>
                  <TableHead>{t('publish.columns.target')}</TableHead>
                  <TableHead>{t('publish.columns.status')}</TableHead>
                  <TableHead className="w-20">{t('publish.columns.trace')}</TableHead>
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
                    <TableCell>
                      {e.kind === 'http' ? (
                        <Switch
                          checked={traceOn('publish', e.local_port)}
                          onCheckedChange={() => toggleTrace('publish', e.local_port)}
                          aria-label={t('publish.trace')}
                        />
                      ) : (
                        <span className="text-xs text-muted-foreground">—</span>
                      )}
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

      {/* 区块二:远程连接(两个操作各走一个对话框) */}
      <Card size="sm">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <ArrowRightLeft className="size-4" /> {t('publish.remoteTitle')}
          </CardTitle>
          <CardDescription>{t('publish.remoteDesc')}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-5">
          {/* 2a. 连接远程端口 */}
          <section className="space-y-3">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h3 className="text-sm font-medium">{t('publish.mirrors.title')}</h3>
                <p className="text-xs text-muted-foreground">{t('publish.mirrors.desc')}</p>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <Badge variant="outline">
                  {t('publish.summary.mirrors', { n: mirrors.length })}
                </Badge>
                <Button variant="outline" size="sm" onClick={openMirror} className="gap-1">
                  <Plus className="size-3.5" /> {t('publish.mirrors.confirm')}
                </Button>
              </div>
            </div>
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

          {/* 2b. 允许远程连接的本地端口 */}
          <section className="border-t pt-5">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0">
                <h3 className="flex items-center gap-2 text-sm font-medium">
                  <Radio className="size-3.5" /> {t('publish.inbound.title')}
                </h3>
                <p className="text-xs text-muted-foreground">{modeSummary}</p>
                {reachable.length > 0 ? (
                  <div className="mt-2 space-y-1.5">
                    {reachable.map((port) => {
                      const info = listeningByPort.get(port)
                      return (
                        <div key={port} className="flex items-center gap-3">
                          <span className="w-14 font-mono text-xs tabular-nums">{port}</span>
                          {info?.http_hint ? (
                            <label className="flex cursor-pointer items-center gap-1.5">
                              <Switch
                                checked={traceOn('p2p', port)}
                                onCheckedChange={() => toggleTrace('p2p', port)}
                                aria-label={t('publish.trace')}
                              />
                              <span className="text-xs text-muted-foreground">
                                {t('publish.traceToggle')}
                              </span>
                            </label>
                          ) : (
                            <span className="text-xs text-muted-foreground">
                              {t('publish.notHttp')}
                            </span>
                          )}
                        </div>
                      )
                    })}
                  </div>
                ) : (
                  policyMode === 'selected' && (
                    <p className="mt-1 text-xs text-muted-foreground">
                      {t('publish.inbound.emptySelected')}
                    </p>
                  )
                )}
              </div>
              <Button variant="outline" size="sm" onClick={openRule} className="shrink-0 gap-1">
                <Settings2 className="size-3.5" /> {t('publish.inbound.configure')}
              </Button>
            </div>
          </section>
        </CardContent>
      </Card>

      {/* 发布端口对话框 */}
      <Dialog open={publishOpen} onOpenChange={setPublishOpen}>
        <DialogContent className="sm:max-w-3xl">
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
                <div className="max-h-64 overflow-auto rounded-md border">
                  {available.map((p) => (
                    <PortRow
                      key={p.port}
                      port={p.port}
                      process={p.process}
                      hint={protocolHint(p)}
                      selected={selPort === p.port}
                      onSelect={() => pickPublishPort(p)}
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
            <Button variant="outline" onClick={() => setPublishOpen(false)}>
              {tc('action.cancel')}
            </Button>
            <Button
              onClick={submitPublish}
              disabled={selPort == null || publishing}
              className="gap-1"
            >
              {publishing && <Loader2 className="size-4 animate-spin" />}
              {t('publish.dialog.confirm')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 连接远程端口对话框:ACCOUNT → 查询 → 选服务 → 本地端口 → 连接 */}
      <Dialog open={mirrorOpen} onOpenChange={setMirrorOpen}>
        <DialogContent className="sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>{t('publish.mirrors.dialogTitle')}</DialogTitle>
            <DialogDescription>{t('publish.mirrors.desc')}</DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="mirror-account">{t('publish.mirrors.accountLabel')}</Label>
              <div className="flex items-center gap-2">
                <input
                  id="mirror-account"
                  className={cn(INPUT_CLS, 'max-w-xs')}
                  value={account}
                  onChange={(e) => setAccount(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && queryServices()}
                  placeholder={t('publish.mirrors.accountPlaceholder')}
                />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={queryServices}
                  disabled={querying || !account.trim()}
                  className="gap-1"
                >
                  {querying ? (
                    <Loader2 className="size-3.5 animate-spin" />
                  ) : (
                    <Search className="size-3.5" />
                  )}
                  {querying ? t('publish.mirrors.querying') : t('publish.mirrors.query')}
                </Button>
              </div>
            </div>

            <div className="space-y-2">
              <p className="text-sm font-medium">{t('publish.mirrors.services')}</p>
              {services == null ? (
                <p className="text-xs text-muted-foreground">{t('publish.mirrors.queryHint')}</p>
              ) : services.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  {t('publish.mirrors.noServices')}
                </p>
              ) : (
                <div className="max-h-56 overflow-auto rounded-md border">
                  {services.map((s) => (
                    <button
                      key={s}
                      type="button"
                      onClick={() => pickService(s)}
                      aria-pressed={selService === s}
                      className={cn(
                        'flex w-full items-center gap-3 border-b px-3 py-2 text-left transition-colors last:border-b-0',
                        selService === s ? 'bg-accent text-accent-foreground' : 'hover:bg-muted'
                      )}
                    >
                      <span className="flex w-5 shrink-0 justify-center">
                        {selService === s && <Check className="size-4" />}
                      </span>
                      <span className="min-w-0 flex-1 truncate font-mono text-xs">{s}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="mirror-port">{t('publish.mirrors.localPortLabel')}</Label>
              <input
                id="mirror-port"
                className={cn(INPUT_CLS, 'max-w-xs')}
                type="number"
                min="1"
                max="65535"
                value={localPort}
                onChange={(e) => setLocalPort(e.target.value)}
                placeholder={t('publish.mirrors.localPortPlaceholder')}
              />
            </div>
          </div>
          {mirrorWarn && <p className="text-xs text-destructive">{mirrorWarn}</p>}

          <DialogFooter>
            <Button variant="outline" onClick={() => setMirrorOpen(false)}>
              {tc('action.cancel')}
            </Button>
            <Button
              onClick={submitMirror}
              disabled={!selService || !localPort.trim() || publishing}
              className="gap-1"
            >
              {publishing && <Loader2 className="size-4 animate-spin" />}
              {t('publish.mirrors.confirm')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 允许远程连接的本地端口:规则对话框 */}
      <Dialog open={ruleOpen} onOpenChange={setRuleOpen}>
        <DialogContent className="sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>{t('publish.inbound.dialogTitle')}</DialogTitle>
            <DialogDescription>{t('publish.inbound.desc')}</DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <Tabs value={ruleMode} onValueChange={setRuleMode}>
              <TabsList>
                <TabsTrigger value="all">{t('publish.inbound.all')}</TabsTrigger>
                <TabsTrigger value="selected">{t('publish.inbound.selected')}</TabsTrigger>
                <TabsTrigger value="none">{t('publish.inbound.none')}</TabsTrigger>
              </TabsList>
            </Tabs>

            {ruleMode === 'all' && (
              <p className="text-xs text-muted-foreground">
                {t('publish.inbound.allHint', { n: listening.length })}
              </p>
            )}
            {ruleMode === 'none' && (
              <p className="text-xs text-muted-foreground">{t('publish.inbound.noneHint')}</p>
            )}
            {ruleMode === 'selected' && (
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-3">
                  <span className="text-xs text-muted-foreground">
                    {t('publish.inbound.selectedSummary', { n: rulePorts.length })}
                  </span>
                  <div className="flex items-center gap-1">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setRulePorts(listening.map((p) => p.port))}
                    >
                      {t('publish.inbound.selectAll')}
                    </Button>
                    <Button variant="ghost" size="sm" onClick={() => setRulePorts([])}>
                      {t('publish.inbound.clear')}
                    </Button>
                  </div>
                </div>
                {listening.length === 0 ? (
                  <p className="text-xs text-muted-foreground">{t('publish.inbound.noPorts')}</p>
                ) : (
                  <div className="max-h-64 overflow-auto rounded-md border">
                    {listening.map((p) => (
                      <PortRow
                        key={p.port}
                        port={p.port}
                        process={p.process}
                        hint={protocolHint(p)}
                        selected={rulePorts.includes(p.port)}
                        onSelect={() => toggleRulePort(p.port)}
                      />
                    ))}
                  </div>
                )}
                {rulePorts.some((p) => !listeningSet.has(p)) && (
                  <p className="text-xs text-muted-foreground">
                    {t('publish.inbound.offlinePorts', {
                      ports: rulePorts.filter((p) => !listeningSet.has(p)).join(', '),
                    })}
                  </p>
                )}
              </div>
            )}
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setRuleOpen(false)}>
              {tc('action.cancel')}
            </Button>
            <Button onClick={submitRule} disabled={publishing} className="gap-1">
              {publishing && <Loader2 className="size-4 animate-spin" />}
              {t('publish.inbound.save')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
