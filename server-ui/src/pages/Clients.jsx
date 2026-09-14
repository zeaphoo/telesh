import { Server } from 'lucide-react'

import { Badge } from '../components/ui/badge'
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
import { formatDuration } from '../lib/utils'
import { useDashboard } from '../store/useDashboard'

export default function Clients() {
  const { clients, error } = useDashboard()

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-semibold">Clients</h1>
        <p className="text-sm text-muted-foreground">
          Agents currently connected to this server
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Online agents</CardTitle>
          <CardDescription>{clients.length} connected</CardDescription>
        </CardHeader>
        <CardContent>
          {error ? (
            <p className="text-sm text-muted-foreground">{error}</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Subdomain</TableHead>
                  <TableHead>Agent ID</TableHead>
                  <TableHead>Online</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {clients.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={3} className="text-muted-foreground">
                      No agents connected.
                    </TableCell>
                  </TableRow>
                ) : (
                  clients.map((c) => (
                    <TableRow key={c.agent_id}>
                      <TableCell className="font-mono text-xs">
                        <Badge variant="success" className="gap-1">
                          <Server className="size-3" />
                          {c.sub_domain}
                        </Badge>
                      </TableCell>
                      <TableCell className="font-mono text-xs">
                        {c.agent_id}
                      </TableCell>
                      <TableCell className="text-muted-foreground">
                        {formatDuration(c.online_secs)}
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  )
}