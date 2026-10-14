import { useTranslation } from 'react-i18next'

import { StatusDot } from '@/components/status-badge'
import { formatAccount } from '@/lib/format'
import { cn } from '@/lib/utils'

const TONE = { online: 'success', connecting: 'warning', offline: 'danger' }

/**
 * navbar 上的账户/登录状态。**全应用只保留这一个状态圆点**;在线时显示
 * `账户句柄 – 显示名`,否则显示状态文案。
 *
 * 这一块**一直显示**(原来挂在 `xl:` 断点上,桌面外壳窗口默认 1120px 就直接看不见)。
 * 窄屏时的退让顺序:先隐显示名(< lg),再把句柄截断;状态圆点与句柄不消失。
 * 显示名可截断,数值不逐秒刷新,避免导航栏左右抖动。
 */
export function AccountIndicator({ account, className }) {
  const { t } = useTranslation()
  const state = account?.state || 'offline'
  const tone = TONE[state] || 'muted'
  const online = state === 'online'
  const label = t(`account.state.${state}`, { defaultValue: state })

  return (
    <span className={cn('inline-flex min-w-0 items-center gap-2', className)}>
      <StatusDot tone={tone} pulse={state !== 'offline'} />
      {online ? (
        <>
          <span
            className="min-w-0 truncate font-mono text-base font-medium tabular-nums"
            title={account?.account || ''}
          >
            {formatAccount(account?.account)}
          </span>
          {account?.display_name && (
            <span className="hidden min-w-0 items-center gap-2 lg:flex">
              <span aria-hidden="true" className="shrink-0 text-muted-foreground/50">
                –
              </span>
              <span
                className="min-w-0 max-w-40 truncate text-base text-muted-foreground"
                title={account.display_name}
              >
                {account.display_name}
              </span>
            </span>
          )}
        </>
      ) : (
        <span className="min-w-0 truncate text-base text-muted-foreground" title={label}>
          {label}
        </span>
      )}
    </span>
  )
}
