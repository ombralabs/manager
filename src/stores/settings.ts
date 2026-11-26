import { defineStore } from 'pinia'
import { ref } from 'vue'
import { useSettingsSaveStore } from './settingsSave'
import { core } from '../api/core'
import { CoreError, toCoreError } from '../api/errors'
import type {
  ApiConfig,
  ApiConfigForm,
  ApiStatus,
  ProfileSettings,
  QuitAction,
} from '../api/types'

/** «Настройки»: Local API, лимит запусков, поведение при закрытии; состояние сервера — и для бокового меню. */
export const useSettingsStore = defineStore('settings', () => {
  const saves = useSettingsSaveStore()
  const api = ref<ApiConfig | null>(null)
  const apiStatus = ref<ApiStatus>({ state: 'off' })
  const profile = ref<ProfileSettings | null>(null)
  /** Прокси для сервисов автоматизации; `null` — сеть компьютера. */
  const servicesProxy = ref<number | null>(null)
  /** Текст ошибки чтения прокси для сервисов (настройка повреждена); остальные настройки от неё не зависят. */
  const servicesProxyError = ref<string | null>(null)
  /** Адрес сервисного аккаунта Google; `null` — ключ не загружен. */
  const googleEmail = ref<string | null>(null)
  /** Сохранённый ключ Google не расшифровался или испорчен: это не «нет ключа» — чтение без ключа не пойдёт. */
  const googleKeyBroken = ref(false)
  const loaded = ref(false)

  function setApi(config: ApiConfig) {
    api.value = config
    apiStatus.value = config.status
  }

  /**
   * Запросы Local API идут строго по одному: форма для `setApiConfig` собирается в момент выполнения
   * из уже обновлённого `api`, иначе второй запрос откатил бы поля первого. Ошибка очередь не останавливает.
   */
  let apiQueue: Promise<unknown> = Promise.resolve()
  function serialize<T>(job: () => Promise<T>): Promise<T> {
    const run = apiQueue.then(job)
    apiQueue = run.catch(() => undefined)
    return run
  }

  async function load(): Promise<void> {
    const [, settings, proxy, email] = await Promise.all([
      serialize(async () => {
        const loadedConfig = await core.apiConfig()
        setApi(loadedConfig)
        return loadedConfig
      }),
      core.profileSettings(),
      core.getServicesProxy().then(
        (id) => ({ id, error: null as string | null }),
        (e: unknown) => ({ id: null, error: toCoreError(e).message }),
      ),
      core.googleKeyEmail().then(
        (address) => ({ address, broken: false }),
        () => ({ address: null, broken: true }),
      ),
    ])
    googleEmail.value = email.address
    googleKeyBroken.value = email.broken
    profile.value = settings
    servicesProxy.value = proxy.id
    servicesProxyError.value = proxy.error
    loaded.value = true
  }

  /** Форма — из текущих настроек с правкой: ядро получает её целиком. Ошибка (порт занят) ничего не меняет. */
  function applyApi(patch: Partial<ApiConfigForm>): Promise<ApiConfig> {
    return serialize(async () => {
      const current = api.value
      if (current === null) throw new CoreError('Internal', 'настройки Local API ещё не загружены')
      const config = await core.setApiConfig({
        enabled: current.enabled,
        port: current.port,
        require_key: current.require_key,
        ...patch,
      })
      setApi(config)
      return config
    })
  }

  function regenerateKey(): Promise<ApiConfig> {
    return serialize(async () => {
      const config = await core.regenerateApiKey()
      setApi(config)
      return config
    })
  }

  /** Сегмент «При закрытии приложения» меняется сразу, при ошибке — обратно. */
  async function setOnQuit(action: QuitAction): Promise<void> {
    const before = profile.value?.on_quit
    if (profile.value) profile.value.on_quit = action
    try {
      await core.setOnQuit(action)
    } catch (e) {
      if (profile.value && before !== undefined) profile.value.on_quit = before
      throw e
    }
  }

  /** Выбор запоминается после ответа ядра; при ошибке остаётся прежний. */
  async function setServicesProxy(id: number | null): Promise<void> {
    await core.setServicesProxy(id)
    servicesProxy.value = id
    servicesProxyError.value = null
  }

  /** Файл ключа читает ядро; возвращает адрес аккаунта. */
  async function setGoogleKey(path: string): Promise<void> {
    googleEmail.value = await core.setGoogleKey(path)
    googleKeyBroken.value = false
  }

  async function clearGoogleKey(): Promise<void> {
    await core.clearGoogleKey()
    googleEmail.value = null
    googleKeyBroken.value = false
  }

  /** Событие `api-status`: сервер поднялся, не поднялся или выключен. */
  function setApiStatus(status: ApiStatus) {
    apiStatus.value = status
    if (api.value) api.value.status = status
  }

  return {
    api,
    apiStatus,
    profile,
    servicesProxy,
    servicesProxyError,
    googleEmail,
    googleKeyBroken,
    loaded,
    load,
    applyApi: (patch: Partial<ApiConfigForm>) => saves.run('integrations.api.' + Object.keys(patch).sort().join('.'), () => applyApi(patch)),
    regenerateKey: () => saves.run('integrations.api.key', () => regenerateKey()),
    setOnQuit: (action: QuitAction) => saves.run('profiles.quit', () => setOnQuit(action)),
    setServicesProxy: (id: number | null) => saves.run('integrations.proxy', () => setServicesProxy(id)),
    setGoogleKey: (path: string) => saves.run('integrations.google', () => setGoogleKey(path)),
    clearGoogleKey: () => saves.run('integrations.google', () => clearGoogleKey()),
    setApiStatus,
  }
})
