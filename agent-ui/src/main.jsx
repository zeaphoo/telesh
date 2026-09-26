import React from 'react'
import ReactDOM from 'react-dom/client'
import { ThemeProvider } from 'next-themes'

import { TooltipProvider } from '@/components/ui/tooltip'
import App from './App'
import './i18n'
import './index.css'

ReactDOM.createRoot(document.getElementById('root')).render(
  <React.StrictMode>
    {/* next-themes 写 `.dark` 到 <html>，与 index.html 里的防闪白脚本共用
        同一个 localStorage key（telesh-ui-theme）。 */}
    <ThemeProvider
      attribute="class"
      storageKey="telesh-ui-theme"
      defaultTheme="system"
      enableSystem
      disableTransitionOnChange
    >
      <TooltipProvider delayDuration={200}>
        <App />
      </TooltipProvider>
    </ThemeProvider>
  </React.StrictMode>
)
