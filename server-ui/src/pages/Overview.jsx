import { Activity, ArrowDownUp, Globe, Radio, Server, Waypoints } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { CopyButton } from '../components/copy-button'
import { EmptyState } from '../components/empty-state'
import { PageHeader } from '../components/page-header'
import { StatCard } from '../components/stat-card'
import { StatusBadge } from '../components/status-badge'
import { Button } from '../components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '../components/ui/card'
import { Skeleton } from '../components/ui/skeleton'
import { formatBytes } from '../lib/format'
import { useDashboard } from '../store/useDashboard'

const STAT_COUNT = 6

export default function Overview() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const { metrics, clients, tunnels, error, loading, lastSync } = useDashboard()

  const m = metrics || {}
  const agentsOnline = m.agents_online ?? 0
  const synced = lastSync ? new Date(lastSync).toLocaleTimeString() : '—'
  const firstLoad = loading && !metrics

  return (
    <div className="mx-auto max-w-5xl space-y-6 px-4 py-6 sm:px-6">
      <PageHeader
        title={t('nav.overview')}
        description={t('overview.syncedAt', { time: synced })}
      />

      {error && (
        <Card>
          <CardContent className="text-sm text-muted-foreground">
            {tc('error.api')}: {error}
          </CardContent>
        </Card>
      )}

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {firstLoad
          ? Array.from({ length: STAT_COUNT }).map((_, i) => (
              <Skeleton key={i} className="h-28 rounded-xl" />
            ))
          : (
            <>
              <StatCard
                icon={Server}
                label={t('overview.agentsOnline')}
                value={agentsOnline}
                hint={t('overview.connected', { n: clients.length })}
                tone={agentsOnline === 0 ? 'warning' : 'success'}
              />
              <StatCard
                icon={Waypoints}
                label={t('overview.tunnels')}
                value={(m.http_tunnels ?? 0) + (m.tcp_tunnels ?? 0)}
                hint={t('overview.httpTcp', {
                  http: m.http_tunnels ?? 0,
                  tcp: m.tcp_tunnels ?? 0,
                })}
              />
              <StatCard
                icon={Activity}
                label={t('overview.activeConnections')}
                value={m.active_connections ?? 0}
              />
              <StatCard
                icon={ArrowDownUp}
                label={t('overview.trafficIn')}
                value={formatBytes(m.traffic_in_bytes ?? 0)}
              />
              <StatCard
                icon={ArrowDownUp}
                label={t('overview.trafficOut')}
                value={formatBytes(m.traffic_out_bytes ?? 0)}
              />
              <StatCard
                icon={Globe}
                label={t('overview.httpRequests')}
                value={m.requests_total ?? 0}
              />
            </>
          )}
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">{t('overview.liveTunnels')}</CardTitle>
          <CardDescription>{t('overview.liveTunnelsDesc')}</CardDescription>
        </CardHeader>
        <CardContent>
          {firstLoad ? (
            <div className="space-y-3">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-6 w-full" />
              ))}
            </div>
          ) : tunnels.length === 0 ? (
            <EmptyState icon={Radio} description={t('overview.noTunnels')} />
          ) : (
            <>
              <ul className="divide-y">
                {tunnels.slice(0, 5).map((tn) => (
                  <li key={tn.name} className="flex items-center gap-2 py-2">
                    <StatusBadge
                      tone={tn.type === 'tcp' ? 'warning' : 'success'}
                      label={tc(`kind.${tn.type}`, { defaultValue: tn.type })}
                    />
                    <span className="font-mono text-xs">{tn.remote}</span>
                    <CopyButton value={tn.remote} />
                    <span className="ml-auto text-xs text-muted-foreground">
                      ↑ {formatBytes(tn.in_bytes)} · ↓ {formatBytes(tn.out_bytes)}
                    </span>
                  </li>
                ))}
              </ul>
              {tunnels.length > 5 && (
                <div className="pt-2">
                  <Button asChild variant="ghost" size="sm">
                    <Link to="/tunnels">{t('overview.viewAll')}</Link>
                  </Button>
                </div>
              )}
            </>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
