import { Check, Languages } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { SUPPORTED_LOCALES } from '@/i18n'

const LABELS = { 'zh-CN': '中文', en: 'English' }

/** Language switcher: 中文 / English, persisted in localStorage. */
export function LocaleSwitcher() {
  const { i18n } = useTranslation()

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label="Language">
          <Languages />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {SUPPORTED_LOCALES.map((lng) => (
          <DropdownMenuItem key={lng} onSelect={() => i18n.changeLanguage(lng)}>
            <span className="flex-1">{LABELS[lng] || lng}</span>
            {i18n.language === lng && <Check className="size-4" />}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
