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
import { formatBytes } from '../lib/utils'
import { useDashboard } from '../store/useDashboard'

export default function Tunnels() {
  const { tunnels, error } = useDashboard()

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-semibold">Tunnels</h1>
        <p className="text-sm text-muted-foreground">
          All tunnels registered on this server
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Active tunnels</CardTitle>
          <CardDescription>{tunnels.length} total</CardDescription>
        </CardHeader>
        <CardContent>
          {error ? (
            <p className="text-sm text-muted-foreground">{error}</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Type</TableHead>
                  <TableHead>Public entry</TableHead>
                  <TableHead>Traffic in</TableHead>
                  <TableHead>Traffic out</TableHead>
                  <TableHead>Status</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {tunnels.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={5} className="text-muted-foreground">
                      No tunnels registered.
                    </TableCell>
                  </TableRow>
                ) : (
                  tunnels.map((t) => (
                    <TableRow key={t.name}>
                      <TableCell>
                        <Badge variant={t.type === 'tcp' ? 'secondary' : 'success'}>
                          {t.type}
                        </Badge>
                      </TableCell>
                      <TableCell className="font-mono text-xs">
                        {t.remote}
                      </TableCell>
                      <TableCell>{formatBytes(t.in_bytes)}</TableCell>
                      <TableCell>{formatBytes(t.out_bytes)}</TableCell>
                      <TableCell>
                        <Badge variant="success">online</Badge>
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