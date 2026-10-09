import { useEffect, useState } from 'react'
import { Network, RefreshCw, Server, UserRound } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { AccountIndicator } from './components/account-indicator'
import { AppShell } from './components/app-shell'
import { LocaleSwitcher } from './components/locale-switcher'
import { ThemeToggle } from './components/theme-toggle'
import { Button } from './components/ui/button'
import { Card, CardContent } from './components/ui/card'
import { Toaster } from './components/ui/sonner'
import { useAgent } from './store/useAgent'
import AccountView from './views/AccountView'
import PublishView from './views/PublishView'
import RequestLogView from './views/RequestLogView'

const NAV = [
  { id: 'publish', labelKey: 'nav.publish', icon: Network },
  { id: 'log', labelKey: 'nav.log', icon: Server },
  { id: 'account', labelKey: 'nav.account', icon: UserRound },
]

export default function App() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const {
    publish,
    account,
    error,
    fetchAll,
    fetchPublish,
    fetchAccount,
    fetchP2pPolicy,
    fetchSettings,
    fetchChannels,
    refreshAll,
  } = useAgent()
  const [tab, setTab] = useState('publish')

  // Poll every 2s while the page is open (requests + ports/publish status).
  useEffect(() => {
    fetchAll()
    fetchPublish()
    fetchAccount()
    fetchP2pPolicy()
    fetchSettings()
    fetchChannels()
    const timer = setInterval(() => {
      fetchAll()
      fetchPublish()
      fetchAccount()
      fetchP2pPolicy()
      fetchSettings()
      fetchChannels()
    }, 2000)
    return () => clearInterval(timer)
  }, [fetchAll, fetchPublish, fetchAccount, fetchP2pPolicy, fetchSettings, fetchChannels])

  const openChannels = publish.filter((e) => e?.state === 'live').length

  const brand = (
    <span className="flex shrink-0 items-center gap-2 text-sm font-semibold tracking-tight">
      <img src="/icon.svg?v=24" alt="" className="size-5" />
      {t('title')}
    </span>
  )

  // 导航栏右侧只保留登录状态与「开启的通道数」(隧道信息可能很多,不再逐个展示)。
  const status = (
    <span className="hidden min-w-0 items-center gap-3 xl:flex">
      <AccountIndicator account={account} />
      <span className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground tabular-nums">
        <Network className="size-3.5 shrink-0" />
        <span className="truncate">{t('header.channels', { n: openChannels })}</span>
      </span>
    </span>
  )

  const actions = (
    <>
      <Button
        variant="ghost"
        size="icon-sm"
        onClick={() => refreshAll()}
        aria-label={tc('action.refresh')}
      >
        <RefreshCw />
      </Button>
      <LocaleSwitcher />
      <ThemeToggle />
    </>
  )

  return (
    <AppShell
      brand={brand}
      status={status}
      actions={actions}
      nav={NAV}
      active={tab}
      onNavigate={(item) => setTab(item.id)}
    >
      {error && (
        <div className="px-4 pt-6 sm:px-6">
          <Card>
            <CardContent className="text-sm text-muted-foreground">
              {tc('error.api')}: {error}
            </CardContent>
          </Card>
        </div>
      )}

      {/* Both views stay mounted so unsaved port/publish drafts survive tab switches. */}
      <div className={tab === 'log' ? undefined : 'hidden'}>
        <RequestLogView />
      </div>
      <div className={tab === 'publish' ? undefined : 'hidden'}>
        <PublishView />
      </div>
      <div className={tab === 'account' ? undefined : 'hidden'}>
        <AccountView />
      </div>

      <Toaster richColors position="top-right" />
    </AppShell>
  )
}
