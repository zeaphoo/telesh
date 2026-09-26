import { cn } from '@/lib/utils'

/** 空状态:图标 + 标题 + 说明 + 可选操作。替代裸的一行灰字。 */
export function EmptyState({ icon: Icon, title, description, action, className }) {
  return (
    <div
      className={cn(
        'flex flex-col items-center justify-center gap-2 px-6 py-10 text-center',
        className
      )}
    >
      {Icon && <Icon className="size-6 text-muted-foreground/50" />}
      {title && <p className="text-sm font-medium">{title}</p>}
      {description && <p className="max-w-sm text-sm text-muted-foreground">{description}</p>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  )
}
