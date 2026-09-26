import { useState } from 'react'
import { Check, Copy } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

/** 一键复制(hostname / 端口 / agent_id)。短暂显示对勾。 */
export function CopyButton({ value, className, size = 'icon-xs', ...props }) {
  const { t } = useTranslation('common')
  const [copied, setCopied] = useState(false)

  async function copy() {
    try {
      await navigator.clipboard.writeText(String(value ?? ''))
      setCopied(true)
      setTimeout(() => setCopied(false), 1200)
    } catch {
      /* 剪贴板不可用时静默失败 */
    }
  }

  return (
    <Button
      type="button"
      variant="ghost"
      size={size}
      className={cn('text-muted-foreground hover:text-foreground', className)}
      aria-label={copied ? t('action.copied') : t('action.copy')}
      onClick={copy}
      {...props}
    >
      {copied ? <Check className="text-success" /> : <Copy />}
    </Button>
  )
}
