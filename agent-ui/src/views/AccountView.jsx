import { ShieldCheck } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { CopyButton } from '../components/copy-button'
import { PageHeader } from '../components/page-header'
import { StatusBadge } from '../components/status-badge'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '../components/ui/card'
import { cn } from '../lib/utils'
import { useAgent } from '../store/useAgent'

const TONE = { online: 'success', connecting: 'warning', offline: 'danger' }

function Field({ label, children, mono }) {
  return (
    <div className="grid gap-1 border-b py-2 last:border-b-0 sm:grid-cols-[10rem_1fr] sm:items-center sm:gap-4">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className={cn('min-w-0 break-all', mono ? 'font-mono text-sm' : 'text-base')}>
        {children}
      </dd>
    </div>
  )
}

export default function AccountView() {
  const { t, i18n } = useTranslation()
  const { account } = useAgent()

  const state = account?.state || 'offline'
  const tone = TONE[state] || 'muted'
  const never = t('account.never')

  const fmt = (secs) => (secs ? new Date(secs * 1000).toLocaleString(i18n.language) : never)
  const session = (() => {
    const exp = account?.session_expires_at
    if (!exp) return never
    const mins = Math.round((exp * 1000 - Date.now()) / 60000)
    const when = fmt(exp)
    return mins > 0
      ? `${when}(${t('account.remaining', { mins })})`
      : `${when}(${t('account.expired')})`
  })()

  return (
    <div className="mx-auto max-w-5xl space-y-4 px-4 py-6 sm:px-6">
      <PageHeader title={t('account.title')} description={t('account.description')} />

      {!account ? (
        <Card>
          <CardContent className="text-sm text-muted-foreground">
            {t('account.unavailable')}
          </CardContent>
        </Card>
      ) : (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <ShieldCheck className="size-4" /> {t('account.identity')}
            </CardTitle>
            <CardDescription>{t('account.identityDesc')}</CardDescription>
          </CardHeader>
          <CardContent>
            <dl>
              <Field label={t('account.fields.status')}>
                <StatusBadge
                  tone={tone}
                  label={t(`account.state.${state}`, { defaultValue: state })}
                />
              </Field>
              <Field label={t('account.fields.account')} mono>
                {account.account ? (
                  <span className="inline-flex items-center gap-1">
                    {account.account}
                    <CopyButton value={account.account} />
                  </span>
                ) : (
                  never
                )}
              </Field>
              <Field label={t('account.fields.displayName')}>
                {account.display_name || never}
              </Field>
              <Field label={t('account.fields.controlServer')} mono>
                {account.control_server || never}
              </Field>
              <Field label={t('account.fields.publicKey')} mono>
                {account.public_key ? (
                  <span className="inline-flex items-start gap-1">
                    <span>{account.public_key}</span>
                    <CopyButton value={account.public_key} />
                  </span>
                ) : (
                  never
                )}
              </Field>
              <Field label={t('account.fields.sessionExpires')}>{session}</Field>
              <Field label={t('account.fields.lastLogin')}>{fmt(account.last_login_at)}</Field>
              <Field label={t('account.fields.dataroot')} mono>
                {account.dataroot || never}
              </Field>
              {account.last_error && (
                <Field label={t('account.fields.lastError')}>
                  <span className="text-destructive">{account.last_error}</span>
                </Field>
              )}
            </dl>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
