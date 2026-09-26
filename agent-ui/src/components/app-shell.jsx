import { useState } from 'react'
import { Menu, PanelLeftClose, PanelLeftOpen } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { cn } from '@/lib/utils'

const SIDEBAR_KEY = 'telesh-ui-sidebar'

function NavList({ nav, active, collapsed, onSelect }) {
  const { t } = useTranslation()
  return (
    <nav className="flex flex-col gap-1 p-2">
      {nav.map((item) => {
        const Icon = item.icon
        const isActive = active === item.id
        return (
          <button
            key={item.id}
            type="button"
            onClick={() => onSelect(item)}
            aria-current={isActive ? 'page' : undefined}
            title={collapsed ? t(item.labelKey) : undefined}
            className={cn(
              'flex items-center gap-3 rounded-md py-2 text-sm transition-colors',
              collapsed ? 'justify-center px-0' : 'px-3',
              isActive
                ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground'
                : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-foreground'
            )}
          >
            <Icon className="size-4 shrink-0" />
            {!collapsed && <span className="truncate">{t(item.labelKey)}</span>}
          </button>
        )
      })}
    </nav>
  )
}

/**
 * 应用外壳:顶部 header(品牌 + 状态 + 操作) + 可折叠侧边栏(移动端为抽屉)。
 * 两个 app 各自实现一份,视觉一致但导航驱动方式不同(这里用 state)。
 */
export function AppShell({ brand, status, actions, nav, active, onNavigate, children }) {
  const { t } = useTranslation('common')
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem(SIDEBAR_KEY) === 'collapsed'
  )
  const [mobileOpen, setMobileOpen] = useState(false)

  function toggleCollapsed() {
    setCollapsed((prev) => {
      const next = !prev
      try {
        localStorage.setItem(SIDEBAR_KEY, next ? 'collapsed' : 'expanded')
      } catch {
        /* ignore */
      }
      return next
    })
  }

  function select(item) {
    setMobileOpen(false)
    onNavigate(item)
  }

  return (
    <div className="min-h-screen bg-background text-foreground">
      <header className="sticky top-0 z-40 border-b bg-card">
        <div className="flex h-14 items-center gap-3 px-4">
          <Button
            variant="ghost"
            size="icon-sm"
            className="md:hidden"
            aria-label={t('action.menu')}
            onClick={() => setMobileOpen(true)}
          >
            <Menu />
          </Button>
          {brand}
          <div className="ml-auto flex items-center gap-2">
            {status}
            {actions}
          </div>
        </div>
      </header>

      <div className="flex">
        <aside
          className={cn(
            'sticky top-14 hidden h-[calc(100vh-3.5rem)] shrink-0 flex-col border-r bg-sidebar text-sidebar-foreground md:flex',
            collapsed ? 'w-16' : 'w-56'
          )}
        >
          <NavList nav={nav} active={active} collapsed={collapsed} onSelect={select} />
          <div className="mt-auto p-2">
            <Button
              variant="ghost"
              size="icon-sm"
              className="w-full text-muted-foreground"
              onClick={toggleCollapsed}
              aria-label={collapsed ? t('action.expandSidebar') : t('action.collapseSidebar')}
            >
              {collapsed ? <PanelLeftOpen /> : <PanelLeftClose />}
            </Button>
          </div>
        </aside>

        <main className="min-w-0 flex-1">{children}</main>
      </div>

      <Sheet open={mobileOpen} onOpenChange={setMobileOpen}>
        <SheetContent side="left" className="w-64 bg-sidebar p-0 text-sidebar-foreground">
          <SheetTitle className="sr-only">{t('action.menu')}</SheetTitle>
          <NavList nav={nav} active={active} collapsed={false} onSelect={select} />
        </SheetContent>
      </Sheet>
    </div>
  )
}
