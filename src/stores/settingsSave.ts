import { defineStore } from 'pinia'
import { reactive } from 'vue'

type SaveState = { pending: number; failed: boolean; saved: boolean; revision: number; dirty: boolean }
/** Keep failures per field: saving another setting must not disguise a failed change. */
export const useSettingsSaveStore = defineStore('settings-save', () => {
  const fields = reactive<Record<string, SaveState>>({})
  function field(key: string) { return fields[key] ??= { pending: 0, failed: false, saved: false, revision: 0, dirty: false } }
  async function run<T>(key: string, job: () => Promise<T>): Promise<T> {
    const state = field(key)
    const revision = state.revision
    state.pending++
    try {
      const result = await job()
      state.failed = false
      state.saved = true
      if (state.revision === revision) state.dirty = false
      return result
    } catch (error) {
      state.failed = true
      throw error
    } finally { state.pending-- }
  }
  function edited(key: string) { const state = field(key); state.dirty = true; state.revision++ }
  function invalid(key: string) { field(key).failed = true }
  function reset(key: string) { field(key).failed = false; field(key).dirty = false }
  function status(scope: string) {
    const states = Object.entries(fields).filter(([key]) => key.startsWith(scope + '.')).map(([, state]) => state)
    if (states.some((state) => state.pending)) return 'saving'
    if (states.some((state) => state.failed)) return 'error'
    if (states.some((state) => state.dirty)) return 'unsaved'
    if (states.some((state) => state.saved)) return 'saved'
    return 'automatic'
  }
  return { run, edited, invalid, reset, status }
})
