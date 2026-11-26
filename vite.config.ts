import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

// Tauri ждёт фронтенд на фиксированном порту.
export default defineConfig({
  plugins: [vue()],
  test: { environment: 'happy-dom', setupFiles: ['src/test/setup.ts'] },
  clearScreen: false,
  server: { port: 5173, strictPort: true },
})
