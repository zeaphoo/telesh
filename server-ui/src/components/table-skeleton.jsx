import { Skeleton } from '@/components/ui/skeleton'
import { TableBody, TableCell, TableRow } from '@/components/ui/table'
import { cn } from '@/lib/utils'

/** 表格首屏骨架。列宽按第一列更宽处理,视觉上更接近真实内容。 */
export function TableSkeleton({ columns = 3, rows = 5 }) {
  return (
    <TableBody>
      {Array.from({ length: rows }).map((_, r) => (
        <TableRow key={r}>
          {Array.from({ length: columns }).map((_, c) => (
            <TableCell key={c}>
              <Skeleton className={cn('h-4', c === 0 ? 'w-24' : 'w-16')} />
            </TableCell>
          ))}
        </TableRow>
      ))}
    </TableBody>
  )
}
