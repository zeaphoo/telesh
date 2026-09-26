import { useTranslation } from 'react-i18next'

import { StatusDot } from '@/components/status-badge'
import { cn } from '@/lib/utils'

const TONE = { online: 'success', connecting: 'warning', offline: 'danger' }

/**
 * navbar 上的账户/登录状态。**全应用只保留这一个状态圆点**;在线时显示账户句柄,
 * 否则显示状态文案。整体定宽、文案不逐秒刷新,避免导航栏左右抖动。
 */
export function AccountIndicator({ account, className }) {
  const { t } = useTranslation()
  const state = account?.state || 'offline'
  const tone = TONE[state] || 'muted'
  const online = state === 'online'

  return (
    <span className={cn('inline-flex min-w-28 shrink-0 items-center gap-2', className)}>
      <StatusDot tone={tone} pulse={state !== 'offline'} />
      {online ? (
        <span
          className="min-w-0 truncate font-mono text-sm font-medium"
          title={account?.account || ''}
        >
          {account?.account}
        </span>
      ) : (
        <span className="text-sm text-muted-foreground">
          {t(`account.state.${state}`, { defaultValue: state })}
        </span>
      )}
    </span>
  )
}
