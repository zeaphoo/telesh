import { Check, Monitor, Moon, Sun } from 'lucide-react'
import { useTheme } from 'next-themes'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'

/**
 * Theme mode switcher (light / dark / system). State lives in next-themes,
 * which writes `.dark` on <html> and persists to `telesh-ui-theme`.
 */
export function ThemeToggle() {
  const { theme, setTheme } = useTheme()
  const { t } = useTranslation('common')

  const options = [
    { value: 'light', icon: Sun, label: t('action.themeLight') },
    { value: 'dark', icon: Moon, label: t('action.themeDark') },
    { value: 'system', icon: Monitor, label: t('action.themeSystem') },
  ]

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={t('action.theme')}>
          <Sun className="dark:hidden" />
          <Moon className="hidden dark:block" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {options.map(({ value, icon: Icon, label }) => (
          <DropdownMenuItem key={value} onSelect={() => setTheme(value)}>
            <Icon className="size-4" />
            <span className="flex-1">{label}</span>
            {theme === value && <Check className="size-4" />}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
