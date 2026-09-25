import { useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowRightLeft,
  Check,
  Globe,
  Loader2,
  Network,
  Plus,
  Save,
  Trash2,
} from 'lucide-react'

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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '../components/ui/table'
import { useAgent } from '../store/useAgent'

const KINDS = [
  { value: 'none', label: '不发布' },
  { value: 'http', label: 'HTTP' },
  { value: 'tcp', label: 'TCP' },
  { value: 'p2p', label: 'P2P' },
  { value: 'mirror', label: '镜像' },
]

const KIND_LABEL = Object.fromEntries(KINDS.map((k) => [k.value, k.label]))

const SELECT_CLS = 'h-8 rounded-md border border-input bg-background px-2 text-xs'
const INPUT_CLS = 'h-8 w-full rounded-md border border-input bg-background px-2 text-xs'

const EMPTY_CFG = {
  kind: 'none',
  sub_domain: '',
  remote_port: '',
  name: '',
  remote: '',
}

function stateMeta(state) {
  switch (state) {
    case 'live':
      return { variant: 'success', label: 'live' }
    case 'pending':
      return { variant: 'secondary', label: 'pending' }
    case 'error':
      return { variant: 'destructive', label: 'error' }
    case 'target_gone':
      return { variant: 'outline', label: 'target gone' }
    default:
      return { variant: 'outline', label: state || '—' }
  }
}

function StatusCell({ entry }) {
  if (!entry) return <span className="text-xs text-muted-foreground">—</span>
  const meta = stateMeta(entry.state)
  const detail =
    entry.detail == null
      ? ''
      : typeof entry.detail === 'string'
        ? entry.detail
        : JSON.stringify(entry.detail)
  return (
    <div className="flex min-w-0 flex-col items-start gap-0.5">
      <Badge variant={meta.variant}>{meta.label}</Badge>
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
  const [mirrorForm, setMirrorForm] = useState({ remote: '', local_port: '', secret: '' })
  const [mirrorWarn, setMirrorWarn] = useState(null)
  const seeded = useRef(false)

  // Seed the draft from the server's desired state exactly once, so the 2s
  // poll can refresh runtime status without clobbering in-progress edits.
  useEffect(() => {
    if (seeded.current || !publishLoaded) return
    const next = {}
    const mirrorRows = []
    for (const e of publish) {
      if (!e || e.kind == null || e.local_port == null) continue
      if (e.kind === 'mirror') {
        mirrorRows.push({
          local_port: String(e.local_port),
          remote: e.remote ?? '',
          secret: e.secret ?? '',
        })
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
      if (m.secret?.trim()) entry.secret = m.secret.trim()
      out.push(entry)
    }
    return out
  }, [rows, cfg, mirrors])

  const statusFor = (kind, port) =>
    publish.find((e) => e && e.kind === kind && Number(e.local_port) === Number(port)) || null
  const entriesForPort = (port) =>
    publish.filter((e) => e && Number(e.local_port) === Number(port))

  const addMirror = () => {
    const remote = mirrorForm.remote.trim()
    const secret = mirrorForm.secret.trim()
    const rawPort = mirrorForm.local_port.trim()
    const port = Number(rawPort)
    if (!remote) {
      setMirrorWarn('请填写远端服务 ACCOUNT/NAME')
      return
    }
    if (!rawPort || !Number.isFinite(port) || port <= 0 || port > 65535) {
      setMirrorWarn('请填写有效的本地端口 (1-65535)')
      return
    }
    if (mirrors.some((m) => Number(m.local_port) === port)) {
      setMirrorWarn(`本地端口 ${port} 已在镜像列表中`)
      return
    }
    const row = { local_port: String(port), remote }
    if (secret) row.secret = secret
    setMirrors((m) => [...m, row])
    setMirrorForm({ remote: '', local_port: '', secret: '' })
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
            placeholder="子域,留空随机"
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
            placeholder="公网端口,留空分配"
          />
        )
      case 'p2p':
        return (
          <input
            className={INPUT_CLS}
            value={c.name}
            onChange={(e) => setCfgFor(port, { name: e.target.value })}
            placeholder="服务名"
          />
        )
      case 'mirror':
        return (
          <input
            className={INPUT_CLS}
            value={c.remote}
            onChange={(e) => setCfgFor(port, { remote: e.target.value })}
            placeholder="ACCOUNT/NAME"
          />
        )
      default:
        return <span className="text-xs text-muted-foreground">—</span>
    }
  }

  return (
    <div className="mx-auto max-w-6xl space-y-4 px-6 py-6">
      {publishError && (
        <Card>
          <CardContent className="py-3 text-sm text-muted-foreground">
            发布接口错误: {publishError}
          </CardContent>
        </Card>
      )}

      {/* 1. Local ports */}
      <Card>
        <CardHeader className="flex-row items-center justify-between space-y-0 gap-3">
          <div>
            <CardTitle className="flex items-center gap-2 text-base">
              <Network className="size-4" /> 本机端口
            </CardTitle>
            <CardDescription>
              {rows.length} 个端口 · 选择发布方式并填写参数,然后点“保存并应用”
            </CardDescription>
          </div>
          <Button onClick={() => savePublish(desired)} disabled={publishing} className="gap-1">
            {publishing ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Save className="size-4" />
            )}
            {publishing ? '保存中…' : '保存并应用'}
          </Button>
        </CardHeader>
        <CardContent className="p-0">
          {portsError && (
            <p className="px-6 pb-3 text-xs text-muted-foreground">
              无法读取 /api/ports: {portsError}
            </p>
          )}
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-24">端口</TableHead>
                <TableHead className="w-32">进程</TableHead>
                <TableHead className="w-24">PID</TableHead>
                <TableHead className="w-40">发布方式</TableHead>
                <TableHead className="w-56">参数</TableHead>
                <TableHead>状态</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={6} className="py-8 text-center text-muted-foreground">
                    {portsError ? '端口列表不可用。' : '未发现监听中的本机端口。'}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((r) => {
                  const c = cfgFor(r.port)
                  const status = c.kind === 'none' ? null : statusFor(c.kind, r.port)
                  const existing = c.kind === 'none' ? entriesForPort(r.port) : []
                  return (
                    <TableRow key={r.port}>
                      <TableCell className="font-mono text-xs">
                        {r.port}
                        {r.http_hint && (
                          <span title="疑似 HTTP 服务">
                            <Globe className="ml-1 inline size-3 text-emerald-600" />
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
                              <option key={k.value} value={k.value}>
                                {k.label}
                              </option>
                            ))}
                          </select>
                          <div className="flex flex-wrap items-center gap-1">
                            {r.probed && r.http_hint ? (
                              <>
                                <Badge variant="success">检测到 HTTP</Badge>
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
                              c.kind !== 'http' && <Badge variant="success">建议 HTTP</Badge>
                            )}
                            {!r.present && <Badge variant="outline">未监听</Badge>}
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
                                <Badge variant="outline">{KIND_LABEL[e.kind] || e.kind}</Badge>
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
            <ArrowRightLeft className="size-4" /> 镜像远端服务
          </CardTitle>
          <CardDescription>
            把远端服务镜像到本地端口:填写 ACCOUNT/NAME 与本地监听端口后加入列表(服务端要求
            lookup secret 时一并填写)
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex flex-wrap items-center gap-2">
            <input
              className={`${INPUT_CLS} w-56`}
              value={mirrorForm.remote}
              onChange={(e) => setMirrorForm((f) => ({ ...f, remote: e.target.value }))}
              placeholder="ACCOUNT/NAME"
            />
            <input
              className={`${INPUT_CLS} w-36`}
              type="number"
              min="1"
              max="65535"
              value={mirrorForm.local_port}
              onChange={(e) => setMirrorForm((f) => ({ ...f, local_port: e.target.value }))}
              placeholder="本地端口"
            />
            <input
              className={`${INPUT_CLS} w-56`}
              type="password"
              autoComplete="off"
              value={mirrorForm.secret}
              onChange={(e) => setMirrorForm((f) => ({ ...f, secret: e.target.value }))}
              placeholder="lookup secret(服务端要求时填)"
            />
            <Button variant="outline" size="sm" onClick={addMirror} className="gap-1">
              <Plus className="size-3.5" /> 加入列表
            </Button>
          </div>
          {mirrorWarn && <p className="text-xs text-red-600">{mirrorWarn}</p>}

          {mirrors.length === 0 ? (
            <p className="py-4 text-center text-sm text-muted-foreground">
              尚未添加镜像。加入后点上方“保存并应用”生效。
            </p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>远端服务</TableHead>
                  <TableHead className="w-32">本地端口</TableHead>
                  <TableHead>状态</TableHead>
                  <TableHead className="w-16" />
                </TableRow>
              </TableHeader>
              <TableBody>
                {mirrors.map((m, i) => (
                  <TableRow key={`${m.local_port}-${i}`}>
                    <TableCell className="font-mono text-xs">
                      <div className="flex items-center gap-1.5">
                        <span>{m.remote || '—'}</span>
                        {m.secret && (
                          <Badge variant="outline" title="已设置 lookup secret">
                            密码
                          </Badge>
                        )}
                      </div>
                    </TableCell>
                    <TableCell className="font-mono text-xs">{m.local_port}</TableCell>
                    <TableCell>
                      <StatusCell entry={statusFor('mirror', m.local_port)} />
                    </TableCell>
                    <TableCell>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="gap-1 text-muted-foreground"
                        onClick={() => setMirrors((list) => list.filter((_, j) => j !== i))}
                      >
                        <Trash2 className="size-3.5" /> 移除
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      {/* Save / status footer */}
      <Card>
        <CardContent className="flex flex-wrap items-center gap-3 py-3">
          <Button onClick={() => savePublish(desired)} disabled={publishing} className="gap-1">
            {publishing ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Save className="size-4" />
            )}
            {publishing ? '保存中…' : '保存并应用'}
          </Button>
          <span className="text-xs text-muted-foreground">
            待发布 {desired.length} 项 · 状态每 2 秒自动刷新
          </span>
          {lastSavedAt && !publishError && (
            <Badge variant="success">
              <Check /> 已保存 {new Date(lastSavedAt).toLocaleTimeString()}
            </Badge>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
