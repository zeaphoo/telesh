import {
  Activity,
  ArrowDownUp,
  Globe,
  Radio,
  Server,
  Waypoints,
} from 'lucide-react'

import { Badge } from '../components/ui/badge'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '../components/ui/card'
import { formatBytes } from '../lib/utils'
import { useDashboard } from '../store/useDashboard'

function StatCard({ icon: Icon, label, value, hint }) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground">
          {label}
        </CardTitle>
        <Icon className="size-4 text-muted-foreground" />
      </CardHeader>
      <CardContent>
        <div className="text-2xl font-semibold">{value}</div>
        {hint && <p className="text-xs text-muted-foreground mt-1">{hint}</p>}
      </CardContent>
    </Card>
  )
}

export default function Overview() {
  const { metrics, clients, tunnels, error, lastSync } = useDashboard()

  if (error) {
    return (
      <div className="space-y-4">
        <h1 className="text-xl font-semibold">Overview</h1>
        <Card>
          <CardContent className="py-8 text-center text-sm text-muted-foreground">
            Cannot reach the server API: {error}
          </CardContent>
        </Card>
      </div>
    )
  }

  const m = metrics || {}
  const synced = lastSync
    ? new Date(lastSync).toLocaleTimeString()
    : '—'

  return (
    <div className="space-y-6">
      <div className="flex items-baseline justify-between">
        <h1 className="text-xl font-semibold">Overview</h1>
        <p className="text-xs text-muted-foreground">
          synced at {synced} · auto-refresh 3s
        </p>
      </div>

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <StatCard
          icon={Server}
          label="Agents online"
          value={m.agents_online ?? 0}
          hint={`${clients.length} connected`}
        />
        <StatCard
          icon={Waypoints}
          label="Tunnels"
          value={(m.http_tunnels ?? 0) + (m.tcp_tunnels ?? 0)}
          hint={`${m.http_tunnels ?? 0} http · ${m.tcp_tunnels ?? 0} tcp`}
        />
        <StatCard
          icon={Activity}
          label="Active connections"
          value={m.active_connections ?? 0}
        />
        <StatCard
          icon={ArrowDownUp}
          label="Traffic in"
          value={formatBytes(m.traffic_in_bytes ?? 0)}
        />
        <StatCard
          icon={ArrowDownUp}
          label="Traffic out"
          value={formatBytes(m.traffic_out_bytes ?? 0)}
        />
        <StatCard
          icon={Globe}
          label="HTTP requests"
          value={m.requests_total ?? 0}
        />
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Live tunnels</CardTitle>
          <CardDescription>Current http/tcp tunnels on this server</CardDescription>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground">
          {tunnels.length === 0 ? (
            <p>No tunnels right now. Run an agent to see it here.</p>
          ) : (
            <ul className="divide-y">
              {tunnels.slice(0, 8).map((t) => (
                <li key={t.name} className="flex items-center gap-2 py-2">
                  <Radio className="size-3.5 text-muted-foreground" />
                  <span className="font-mono text-xs">{t.remote}</span>
                  <Badge variant={t.type === 'tcp' ? 'secondary' : 'success'}>
                    {t.type}
                  </Badge>
                  <span className="ml-auto text-xs">
                    ↑ {formatBytes(t.in_bytes)} · ↓ {formatBytes(t.out_bytes)}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>
    </div>
  )
}