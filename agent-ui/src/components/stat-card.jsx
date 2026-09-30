import { Card, CardAction, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { cn } from '@/lib/utils'

const TONE_VALUE = {
  success: 'text-success',
  warning: 'text-warning',
  danger: 'text-destructive',
  info: 'text-info',
}

/** 指标卡:标签 + 图标 + 大数字 + 可选提示。数字用等宽数字防止跳动。 */
export function StatCard({ icon: Icon, label, value, hint, tone, className }) {
  return (
    <Card size="sm" className={className}>
      <CardHeader>
        <CardTitle className="text-sm font-medium text-muted-foreground">{label}</CardTitle>
        {Icon && (
          <CardAction>
            <Icon className="size-4 text-muted-foreground" />
          </CardAction>
        )}
      </CardHeader>
      <CardContent>
        <div className={cn('text-2xl font-semibold tabular-nums', tone && TONE_VALUE[tone])}>
          {value}
        </div>
        {hint && <p className="mt-1 text-xs text-muted-foreground">{hint}</p>}
      </CardContent>
    </Card>
  )
}
