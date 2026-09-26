import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { StatusDot } from '@/components/status-badge'
import { cn } from '@/lib/utils'

/**
 * "已连接 · 2s 前" 指示器。每秒重渲染一次以刷新相对时间。
 * `lastSync` 是最近一次成功拉取的毫秒时间戳。
 */
export function LiveIndicator({ online = true, lastSync, className }) {
  const { t } = useTranslation('common')
  const [, tick] = useState(0)

  useEffect(() => {
    const id = setInterval(() => tick((n) => n + 1), 1000)
    return () => clearInterval(id)
  }, [])

  const seconds = lastSync ? Math.max(0, Math.floor((Date.now() - lastSync) / 1000)) : null
  const label = !online
    ? t('live.disconnected')
    : seconds == null
      ? t('live.connected')
      : t('live.connectedAgo', { seconds })

  return (
    <span
      className={cn('inline-flex items-center gap-1.5 text-xs text-muted-foreground', className)}
    >
      <StatusDot tone={online ? 'success' : 'danger'} pulse={online} />
      {label}
    </span>
  )
}
