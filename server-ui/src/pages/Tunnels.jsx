import { Waypoints } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { CopyButton } from '../components/copy-button'
import { EmptyState } from '../components/empty-state'
import { PageHeader } from '../components/page-header'
import { StatusBadge } from '../components/status-badge'
import { TableSkeleton } from '../components/table-skeleton'
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
import { formatBytes } from '../lib/format'
import { useDashboard } from '../store/useDashboard'

export default function Tunnels() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const { tunnels, error, loading } = useDashboard()

  return (
    <div className="mx-auto max-w-5xl space-y-4 px-4 py-6 sm:px-6">
      <PageHeader title={t('nav.tunnels')} description={t('tunnels.description')} />

      <Card>
        <CardHeader>
          <CardTitle className="text-base">{t('tunnels.title')}</CardTitle>
          <CardDescription>{t('tunnels.count', { n: tunnels.length })}</CardDescription>
        </CardHeader>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('tunnels.columns.type')}</TableHead>
                <TableHead>{t('tunnels.columns.publicEntry')}</TableHead>
                <TableHead className="text-right">{t('tunnels.columns.trafficIn')}</TableHead>
                <TableHead className="text-right">{t('tunnels.columns.trafficOut')}</TableHead>
                <TableHead>{t('tunnels.columns.status')}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {loading && !error && tunnels.length === 0 ? (
                <TableSkeleton columns={5} />
              ) : error || tunnels.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={5} className="p-0">
                    <EmptyState
                      icon={Waypoints}
                      description={
                        error ? `${tc('error.api')}: ${error}` : t('tunnels.empty')
                      }
                    />
                  </TableCell>
                </TableRow>
              ) : (
                tunnels.map((tn) => (
                  <TableRow key={tn.name}>
                    <TableCell>
                      <StatusBadge
                        tone={tn.type === 'tcp' ? 'warning' : 'success'}
                        label={tc(`kind.${tn.type}`, { defaultValue: tn.type })}
                      />
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      <div className="flex items-center gap-1">
                        <span>{tn.remote}</span>
                        <CopyButton value={tn.remote} />
                      </div>
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatBytes(tn.in_bytes)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {formatBytes(tn.out_bytes)}
                    </TableCell>
                    <TableCell>
                      <StatusBadge state="online" />
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  )
}
