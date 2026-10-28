import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// En desarrollo, la API la sirve `huella ui --port 4874 --no-browser` aparte.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': path.resolve(import.meta.dirname, './src') } },
  server: { proxy: { '/api': 'http://127.0.0.1:4874' } },
  build: { outDir: 'dist', emptyOutDir: true, chunkSizeWarningLimit: 900 },
})
