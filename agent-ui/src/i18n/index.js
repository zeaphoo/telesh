import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'

import zhAgent from './locales/zh-CN/agent.json'
import zhCommon from './locales/zh-CN/common.json'
import enAgent from './locales/en/agent.json'
import enCommon from './locales/en/common.json'

const STORAGE_KEY = 'telesh-ui-locale'

/** 支持的语言。新增语言时同时补 locales/<tag>/ 目录。 */
export const SUPPORTED_LOCALES = ['zh-CN', 'en']

/**
 * 浏览器语言优先:zh* -> zh-CN,en* -> en,其余回退 zh-CN。
 * localStorage 里的显式选择优先级最高。
 */
function detectLocale() {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (SUPPORTED_LOCALES.includes(saved)) return saved
  } catch {
    /* localStorage 不可用时忽略 */
  }
  const nav = (navigator.languages?.[0] || navigator.language || '').toLowerCase()
  if (nav.startsWith('zh')) return 'zh-CN'
  if (nav.startsWith('en')) return 'en'
  return 'zh-CN'
}

i18n.use(initReactI18next).init({
  resources: {
    'zh-CN': { common: zhCommon, agent: zhAgent },
    en: { common: enCommon, agent: enAgent },
  },
  lng: detectLocale(),
  fallbackLng: 'zh-CN',
  ns: ['common', 'agent'],
  defaultNS: 'agent',
  interpolation: { escapeValue: false },
})

document.documentElement.lang = i18n.language
i18n.on('languageChanged', (lng) => {
  document.documentElement.lang = lng
  try {
    localStorage.setItem(STORAGE_KEY, lng)
  } catch {
    /* ignore */
  }
})

export default i18n
