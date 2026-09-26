import { useTranslation } from 'react-i18next'

import { Badge } from '@/components/ui/badge'
import { statusMeta } from '@/lib/status'
import { cn } from '@/lib/utils'

/** tone -> 底色/描边/文字。用按透明度的扩展色,深浅两套 token 自动生效。 */
const TONE_BADGE = {
  success: 'border-success/25 bg-success/15 text-success',
  warning: 'border-warning/25 bg-warning/15 text-warning',
  danger: 'border-destructive/25 bg-destructive/15 text-destructive',
  info: 'border-info/25 bg-info/15 text-info',
  muted: 'border-border bg-muted text-muted-foreground',
}

const TONE_DOT = {
  success: 'bg-success',
  warning: 'bg-warning',
  danger: 'bg-destructive',
  info: 'bg-info',
  muted: 'bg-muted-foreground',
}

/** 状态圆点(pending 会呼吸)。颜色之外的第二重表达。 */
export function StatusDot({ tone = 'muted', pulse = false, className }) {
  const color = TONE_DOT[tone] || TONE_DOT.muted
  return (
    <span className={cn('relative inline-flex size-1.5 shrink-0', className)}>
      {pulse && (
        <span
          className={cn(
            'absolute inline-flex size-full animate-ping rounded-full opacity-60 motion-reduce:animate-none',
            color
          )}
        />
      )}
      <span className={cn('relative inline-flex size-full rounded-full', color)} />
    </span>
  )
}

/**
 * 带语义色的状态徽标。`state` 走 statusMeta 映射,`label` 可覆盖文案。
 */
export function StatusBadge({ state, label, className, ...props }) {
  const { t } = useTranslation('common')
  const meta = statusMeta(state)
  const text = label ?? t(meta.key, { defaultValue: state || '—' })

  return (
    <Badge
      variant="outline"
      className={cn('gap-1.5', TONE_BADGE[meta.tone], className)}
      {...props}
    >
      <StatusDot tone={meta.tone} pulse={meta.pulse} />
      {text}
    </Badge>
  )
}
