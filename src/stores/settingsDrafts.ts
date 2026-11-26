import { defineStore } from 'pinia'
import { reactive } from 'vue'
import type { FieldError } from '../api/types'

type DelayKey = 'click_delay' | 'typing_delay'
/** In-memory form state: a section/search transition must not discard a rejected edit or its error. */
export const useSettingsDraftsStore = defineStore('settings-drafts', () => {
  const api = reactive({
    errors: [] as FieldError[], portText: '', acceptedPort: null as number | null,
    failedPort: null as string | null, pendingPort: null as string | null,
  })
  const sync = reactive({
    errors: [] as FieldError[],
    text: { click_delay: ['', ''], typing_delay: ['', ''] } as Record<DelayKey, [string, string]>,
    accepted: { click_delay: null, typing_delay: null } as Record<DelayKey, [number, number] | null>,
  })
  return { api, sync }
})
