import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Dev: vite serves on :16473 and proxies API calls to the running
// telesh-server dashboard (default 127.0.0.1:16456).
// Prod: `vite build` emits dist/, which telesh-server embeds via rust-embed
// and serves same-origin (API + static).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 16473,
    proxy: {
      '/api': 'http://127.0.0.1:16456',
      '/metrics': 'http://127.0.0.1:16456',
    },
  },
  build: {
    outDir: 'dist',
  },
})