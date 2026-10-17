import { useEffect, useState } from 'react'
import { CircleAlert, Network, RefreshCw, Server, TriangleAlert, UserRound } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { AccountIndicator } from './components/account-indicator'
import { AppShell } from './components/app-shell'
import { LocaleSwitcher } from './components/locale-switcher'
import { ThemeToggle } from './components/theme-toggle'
import { Alert, AlertDescription, AlertTitle } from './components/ui/alert'
import { Button } from './components/ui/button'
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

/** 轮询间隔。面板不可达时也是靠它自动恢复的,所以状态条里要把它说清楚。 */
const POLL_MS = 2000

export default function App() {
  const { t } = useTranslation()
  const { t: tc } = useTranslation('common')
  const {
    publish,
    account,
    error,
    agentDown,
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
    }, POLL_MS)
    return () => clearInterval(timer)
  }, [fetchAll, fetchPublish, fetchAccount, fetchP2pPolicy, fetchSettings, fetchChannels])

  const openChannels = publish.filter((e) => e?.state === 'live').length

  const brand = (
    <span className="flex shrink-0 items-center gap-2 text-sm font-semibold tracking-tight">
      <img src="/icon.svg?v=26" alt="" className="size-5" />
      {/* 跟导航项标签同一个退让规则:窄屏只留图标。 */}
      <span className="hidden truncate sm:inline">{t('title')}</span>
    </span>
  )

  // 导航栏右侧只保留登录状态与「开启的通道数」(隧道信息可能很多,不再逐个展示)。
  //
  // 一直显示,但空间不足时按固定优先级退让:**先通道 → 再显示名 → 最后才动账号**
  // (账号那块见 `account-indicator.jsx`)。原来整块挂在 `xl:` 上,桌面外壳窗口
  // 默认 1120px,低于 xl 就什么都看不见。
  const status = (
    <span className="flex min-w-0 items-center gap-3">
      <AccountIndicator account={account} />
      <span className="hidden shrink-0 items-center gap-1 text-xs text-muted-foreground tabular-nums lg:flex">
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
      {/*
        全局错误条:**只此一处**。面板不可达时各功能内的错误一律静默(见 store
        的 `noteFailure`),所以不会出现同一条错在页面上重复三遍。

        做成通栏贴边的状态条,而不是卡片:它是「当前状态」不是一条内容。
        不给重试按钮 —— 轮询本来就会自动恢复,再放个按钮反而让人以为要点它。
      */}
      {(agentDown || error) && (
        <Alert
          variant={agentDown ? 'destructive' : 'default'}
          className={`rounded-none border-x-0 border-t-0 px-4 py-2.5 sm:px-6 ${
            agentDown ? 'border-destructive/30 bg-destructive/10' : 'border-warning/30 bg-warning/10'
          }`}
        >
          {agentDown ? <TriangleAlert /> : <CircleAlert />}
          <AlertTitle>{agentDown ? tc('error.agentDown') : tc('error.api')}</AlertTitle>
          <AlertDescription>
            {agentDown ? tc('error.agentDownHint', { seconds: POLL_MS / 1000 }) : error}
          </AlertDescription>
        </Alert>
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
