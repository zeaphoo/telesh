import { useTranslation } from 'react-i18next'

import { StatusDot } from '@/components/status-badge'
import { cn } from '@/lib/utils'

const TONE = { online: 'success', connecting: 'warning', offline: 'danger' }

/**
 * navbar 上的账户/登录状态。**全应用只保留这一个状态圆点**;在线时显示
 * `账户句柄 – 显示名`,否则显示状态文案。显示名可截断,数值不逐秒刷新,
 * 避免导航栏左右抖动。
 */
export function AccountIndicator({ account, className }) {
  const { t } = useTranslation()
  const state = account?.state || 'offline'
  const tone = TONE[state] || 'muted'
  const online = state === 'online'

  return (
    <span className={cn('inline-flex min-w-0 items-center gap-2', className)}>
      <StatusDot tone={tone} pulse={state !== 'offline'} />
      {online ? (
        <>
          <span className="shrink-0 font-mono text-base font-medium tabular-nums">
            {account?.account}
          </span>
          {account?.display_name && (
            <>
              <span aria-hidden="true" className="shrink-0 text-muted-foreground/50">
                –
              </span>
              <span
                className="min-w-0 max-w-40 truncate text-base text-muted-foreground"
                title={account.display_name}
              >
                {account.display_name}
              </span>
            </>
          )}
        </>
      ) : (
        <span className="shrink-0 text-base text-muted-foreground">
          {t(`account.state.${state}`, { defaultValue: state })}
        </span>
      )}
    </span>
  )
}
