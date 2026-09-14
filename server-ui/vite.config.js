import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Dev: vite serves on :5173 and proxies API calls to the running
// telesh-server dashboard (default 127.0.0.1:7500).
// Prod: `vite build` emits dist/, which telesh-server embeds via rust-embed
// and serves same-origin (API + static).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:7500',
      '/metrics': 'http://127.0.0.1:7500',
    },
  },
  build: {
    outDir: 'dist',
  },
})