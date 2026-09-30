import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router-dom'

import { cn } from '@/lib/utils'

function navItemClass({ isActive }) {
  return cn(
    'inline-flex shrink-0 items-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors',
    isActive
      ? 'bg-accent font-medium text-accent-foreground'
      : 'text-muted-foreground hover:bg-accent/60 hover:text-foreground'
  )
}

function NavList({ nav }) {
  const { t } = useTranslation()
  return (
    <nav className="flex min-w-0 items-center gap-1">
      {nav.map((item) => {
        const Icon = item.icon
        return (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.end}
            aria-label={t(item.labelKey)}
            className={navItemClass}
          >
            <Icon className="size-4 shrink-0" />
            <span className="hidden truncate sm:inline">{t(item.labelKey)}</span>
          </NavLink>
        )
      })}
    </nav>
  )
}

/** 应用外壳:导航、品牌、状态与操作都在顶部 navbar 上。 */
export function AppShell({ brand, status, actions, nav, children }) {
  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="sticky top-0 z-40 border-b bg-card">
        <div className="flex h-12 items-center gap-3 px-3">
          {brand}
          <NavList nav={nav} />
          <div className="ml-auto flex shrink-0 items-center gap-2">
            {status}
            {actions}
          </div>
        </div>
      </header>

      <main className="min-w-0">{children}</main>
    </div>
  )
}
