import { useTranslation } from 'react-i18next'

import { cn } from '@/lib/utils'

function NavList({ nav, active, onSelect }) {
  const { t } = useTranslation()
  return (
    <nav className="flex min-w-0 items-center gap-1">
      {nav.map((item) => {
        const Icon = item.icon
        const isActive = active === item.id
        return (
          <button
            key={item.id}
            type="button"
            onClick={() => onSelect(item)}
            aria-current={isActive ? 'page' : undefined}
            aria-label={t(item.labelKey)}
            className={cn(
              'inline-flex shrink-0 items-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors',
              isActive
                ? 'bg-accent font-medium text-accent-foreground'
                : 'text-muted-foreground hover:bg-accent/60 hover:text-foreground'
            )}
          >
            <Icon className="size-4 shrink-0" />
            <span className="hidden truncate sm:inline">{t(item.labelKey)}</span>
          </button>
        )
      })}
    </nav>
  )
}

/** 应用外壳:导航、品牌、状态与操作都在顶部 navbar 上。 */
export function AppShell({ brand, status, actions, nav, active, onNavigate, children }) {
  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="sticky top-0 z-40 border-b bg-card">
        <div className="flex h-12 items-center gap-3 px-3">
          {brand}
          <NavList nav={nav} active={active} onSelect={onNavigate} />
          <div className="ml-auto flex min-w-0 items-center gap-2">
            {status}
            {actions}
          </div>
        </div>
      </header>

      <main className="min-w-0">{children}</main>
    </div>
  )
}
