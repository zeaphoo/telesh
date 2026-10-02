import { fileURLToPath, URL } from 'node:url'

import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Dev: proxy to the agent's inspector dashboard (default 127.0.0.1:16345).
// Prod: `vite build` -> dist/ embedded in the telesh agent via rust-embed.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    // shadcn/ui 组件统一用 `@/...` 导入。
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  server: {
    port: 16373,
    proxy: {
      '/api': 'http://127.0.0.1:16345',
    },
  },
  build: {
    outDir: 'dist',
  },
})
