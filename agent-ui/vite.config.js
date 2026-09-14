import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Dev: proxy to the agent's inspector dashboard (default 127.0.0.1:7400).
// Prod: `vite build` -> dist/ embedded in the telesh agent via rust-embed.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5138,
    proxy: {
      '/api': 'http://127.0.0.1:7400',
    },
  },
  build: {
    outDir: 'dist',
  },
})