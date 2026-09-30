import { Users } from 'lucide-react'
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
import { formatDuration } from '../lib/format'
import { useDashboard } from '../store/useDashboard'

export default function Clients() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const { clients, error, loading } = useDashboard()

  return (
    <div className="mx-auto max-w-5xl space-y-3 px-4 py-4 sm:px-5">
      <PageHeader title={t('nav.clients')} description={t('clients.description')} />

      <Card size="sm">
        <CardHeader>
          <CardTitle className="text-base">{t('clients.title')}</CardTitle>
          <CardDescription>{t('clients.count', { n: clients.length })}</CardDescription>
        </CardHeader>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('clients.columns.subdomain')}</TableHead>
                <TableHead>{t('clients.columns.agentId')}</TableHead>
                <TableHead>{t('clients.columns.online')}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {loading && !error && clients.length === 0 ? (
                <TableSkeleton columns={3} />
              ) : error || clients.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={3} className="p-0">
                    <EmptyState
                      icon={Users}
                      description={error ? `${tc('error.api')}: ${error}` : t('clients.empty')}
                    />
                  </TableCell>
                </TableRow>
              ) : (
                clients.map((c) => (
                  <TableRow key={c.agent_id}>
                    <TableCell>
                      <div className="flex items-center gap-1">
                        <StatusBadge tone="success" label={c.sub_domain} />
                        <CopyButton value={c.sub_domain} />
                      </div>
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      <div className="flex max-w-64 items-center gap-1">
                        <span className="truncate" title={c.agent_id}>
                          {c.agent_id}
                        </span>
                        <CopyButton value={c.agent_id} />
                      </div>
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      {formatDuration(c.online_secs)}
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
