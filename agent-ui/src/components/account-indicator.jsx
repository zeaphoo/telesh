import { useTranslation } from 'react-i18next'

import { StatusDot } from '@/components/status-badge'
import { cn } from '@/lib/utils'

const TONE = { online: 'success', connecting: 'warning', offline: 'danger' }

/** navbar 上的账户/登录状态小指示器。在线时显示账户句柄,否则显示状态文案。 */
export function AccountIndicator({ account, className }) {
  const { t } = useTranslation()
  const state = account?.state || 'offline'
  const tone = TONE[state] || 'muted'
  const label =
    state === 'online'
      ? account?.account || t('account.state.online')
      : t(`account.state.${state}`, { defaultValue: state })

  return (
    <span
      className={cn('inline-flex items-center gap-1.5 text-xs text-muted-foreground', className)}
    >
      <StatusDot tone={tone} pulse={state !== 'offline'} />
      <span className="font-mono">{label}</span>
    </span>
  )
}
