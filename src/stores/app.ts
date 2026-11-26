import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { core } from '../api/core'
import type { AppInfo, Language } from '../api/types'
import { setLocale } from '../i18n'

export const useAppStore = defineStore('app', () => {
  const info = ref<AppInfo | null>(null)
  const importing = ref(false)
  const engineVersion = computed(() => info.value?.engine?.version ?? null)

  async function load() {
    info.value = await core.appInfo()
    setLocale(info.value.language)
  }

  /** Импорт архива движка; после ответа ядра в `info` новая сборка. */
  async function importEngine(path: string) {
    importing.value = true
    try {
      const build = await core.importEngine(path)
      if (info.value) info.value.engine = build
    } finally {
      importing.value = false
    }
  }

  async function setLanguage(language: Language) {
    await core.setLanguage(language)
    setLocale(language)
    if (info.value) info.value.language = language
  }

  return { info, importing, engineVersion, load, importEngine, setLanguage }
})
