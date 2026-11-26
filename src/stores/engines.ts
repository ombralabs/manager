import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { core } from '../api/core'
import { toCoreError } from '../api/errors'
import type { EngineCatalog, EngineProgress, EngineUpdateOutcome } from '../api/types'
import { useAppStore } from './app'
import { useSettingsSaveStore } from './settingsSave'

export const useEnginesStore = defineStore('engines', () => {
  const app = useAppStore()
  const saves = useSettingsSaveStore()
  const catalog = ref<EngineCatalog | null>(null)
  const loading = ref(false)
  const operation = ref<'update' | 'import' | 'select' | 'settings' | null>(null)
  const progress = ref<EngineProgress | null>(null)
  const errorCode = ref<string | null>(null)
  const result = ref<EngineUpdateOutcome['state'] | 'cancelled' | null>(null)
  const busy = computed(() => operation.value !== null || app.importing)
  let startupChecked = false
  let revision = 0

  function apply(next: EngineCatalog) {
    catalog.value = next
    const active = next.versions.find((version) => version.active)
    if (app.info) app.info.engine = active ? { version: active.chromium, binary: active.binary } : null
  }

  async function load() {
    const ticket = ++revision
    loading.value = true
    try {
      const next = await core.engineCatalog()
      if (ticket === revision) apply(next)
    } catch (error) {
      if (ticket === revision) errorCode.value = toCoreError(error).code
    } finally {
      loading.value = false
    }
  }

  function failure(error: unknown) {
    const code = toCoreError(error).code
    if (code === 'EngineUpdateCancelled') result.value = 'cancelled'
    else errorCode.value = code
  }

  async function update(background = false) {
    if (busy.value) return
    revision += 1
    operation.value = 'update'
    errorCode.value = null
    result.value = null
    progress.value = { phase: 'checking', version: null, downloaded: 0, total: null }
    try {
      const outcome = await core.updateEngine(background)
      revision += 1
      apply(outcome.catalog)
      result.value = outcome.state
    } catch (error) {
      failure(error)
    } finally {
      progress.value = null
      operation.value = null
    }
  }

  async function startup() {
    if (startupChecked) return
    startupChecked = true
    await load()
    if (catalog.value?.automatic) await update(true)
  }

  async function change(action: () => Promise<unknown>, kind: 'import' | 'select' | 'settings') {
    if (busy.value) return false
    revision += 1
    operation.value = kind
    errorCode.value = null
    result.value = null
    try {
      await action()
      await load()
      return true
    } catch (error) {
      failure(error)
      return false
    } finally {
      operation.value = null
    }
  }

  async function automatic(enabled: boolean) {
    if (await change(() => saves.run('engine.automatic', () => core.setEngineAutomatic(enabled)), 'settings')) {
      if (enabled) await update(true)
    }
  }

  async function cancel() {
    try { await core.cancelEngineUpdate() } catch (error) { failure(error) }
  }

  function onProgress(event: EngineProgress) {
    if (operation.value === 'update') progress.value = event
  }

  return {
    catalog, loading, operation, progress, errorCode, result, busy, load, startup, update, cancel, automatic, onProgress,
    select: (id: string) => change(() => saves.run('engine.version', () => core.selectEngine(id)), 'select'),
    importArchive: (path: string) => change(() => app.importEngine(path), 'import'),
  }
})
