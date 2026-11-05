import { defineConfig } from 'electron-vite'
import react from '@vitejs/plugin-react'
import { resolve } from 'node:path'

const alias = { '@shared': resolve(__dirname, 'src/shared') }

export default defineConfig({
  main: {
    resolve: { alias },
    build: { rollupOptions: { input: { index: resolve(__dirname, 'src/main/index.ts') } } },
  },
  preload: {
    resolve: { alias },
    build: { rollupOptions: { input: { index: resolve(__dirname, 'src/preload/index.ts') } } },
  },
  renderer: {
    root: resolve(__dirname, 'src/renderer'),
    resolve: { alias },
    plugins: [react()],
    build: { rollupOptions: { input: { index: resolve(__dirname, 'src/renderer/index.html') } } },
  },
})
