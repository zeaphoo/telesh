import { cn } from '@/lib/utils'

/** 页面标题区:标题 + 描述 + 右侧操作。 */
export function PageHeader({ title, description, actions, className }) {
  return (
    <div className={cn('flex flex-wrap items-start justify-between gap-3', className)}>
      <div className="space-y-1">
        <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
        {description && <p className="text-sm text-muted-foreground">{description}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </div>
  )
}
