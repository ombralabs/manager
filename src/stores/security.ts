import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { core } from '../api/core'
import type { SecurityStatus } from '../api/types'
import { toCoreError } from '../api/errors'
import { i18n } from '../i18n'

/** Состояние интерфейса: разрешение на чтение данных остаётся исключительно в нативном ядре. */
export const useSecurityStore = defineStore('security', () => {
  const status = ref<SecurityStatus | null>(null)
  const error = ref('')
  const busy = ref(false)
  const unlocked = computed(() => status.value?.unlocked === true)
  const available = computed(() => status.value !== null && (!status.value.configured || status.value.unlocked))
  async function load() { status.value = await core.securityStatus() }
  async function run<T>(action: () => Promise<T>): Promise<T> {
    busy.value = true
    error.value = ''
    try { return await action() }
    catch (e) {
      const failure = toCoreError(e)
      const key = `errors.${failure.code}`
      error.value = i18n.global.te(key) ? i18n.global.t(key) : failure.message
      throw e
    } finally {
      await load().catch(() => {})
      busy.value = false
    }
  }
  return { status, error, busy, unlocked, available, load, run }
})
