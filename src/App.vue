<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core, events } from './api/core'
import type { SupervisorEvent } from './api/types'
import AppSidebar from './components/AppSidebar.vue'
import BulkReportDialog from './components/BulkReportDialog.vue'
import ConfirmDialog from './components/ConfirmDialog.vue'
import FatalScreen from './components/FatalScreen.vue'
import ToastHost from './components/ToastHost.vue'
import { notify } from './lib/notify'
import { createSupervisorNotices } from './lib/supervisorNotices'
import { useAppStore } from './stores/app'
import { useEnginesStore } from './stores/engines'
import { currentReport, profileName } from './lib/bulkReport'
import { viewKey } from './router'
import { useAutomationStore } from './stores/automation'
import { useSyncStore } from './stores/sync'
import { useGroupsStore } from './stores/groups'
import { useProfilesStore } from './stores/profiles'
import { useProfileLabelsStore } from './stores/profileLabels'
import { useProxiesStore } from './stores/proxies'
import { useSettingsStore } from './stores/settings'
import { useSecurityStore } from './stores/security'
import SecurityGate from './views/SecurityGate.vue'

const app = useAppStore()
const engines = useEnginesStore()
const profiles = useProfilesStore()
const profileLabels = useProfileLabelsStore()
const groups = useGroupsStore()
const settings = useSettingsStore()
const security = useSecurityStore()
const proxies = useProxiesStore()
const automation = useAutomationStore()
const sync = useSyncStore()
const { t } = useI18n()
const notices = createSupervisorNotices({
  nameOf: profileName,
  fetchName: (id) => core.getProfile(id).then((details) => details.profile.name),
})
const ready = ref(false)
const subscribed = ref(false)
const quitOpen = ref(false)
const quitActive = ref(0)
const quitting = ref(false)
const unlisten: UnlistenFn[] = []
const workspaceAvailable = computed(() => ready.value && security.available)

watch(workspaceAvailable, () => {
  currentReport.value = null
}, { immediate: true, flush: 'sync' })

/** Прокси перечитываются, только если список уже загружали: экран «Прокси» и вкладка «Шлюз» при открытии читают его сами. */
function refreshProxies() {
  if (proxies.loaded) proxies.scheduleRefresh()
}

function onSupervisor(event: SupervisorEvent) {
  profiles.applyEvent(event)
  groups.applyEvent(event)
  // Запуск и остановка меняют «работает: <имя>», замена шлюза — «Закреплён за».
  refreshProxies()
  if (workspaceAvailable.value) void notices.handle(event)
}

/** Local API изменил профили, группы или прокси: списки перезапрашиваются не чаще раза в 500 мс. */
function onProfilesChanged() {
  profileLabels.scheduleRefresh()
  profiles.scheduleRefresh()
  groups.scheduleRefresh()
  refreshProxies()
}

/** Неудачная подписка не мешает остальным; её ошибка — уведомлением. */
async function subscribe(listen: () => Promise<UnlistenFn>) {
  try {
    unlisten.push(await listen())
  } catch (e) {
    notify.error(e)
  }
}

async function quit() {
  quitting.value = true
  try {
    await core.confirmQuit()
  } catch (e) {
    quitting.value = false
    notify.error(e)
  }
}

onMounted(async () => {
  try {
    await app.load()
    if (!app.info?.init_error) await security.load()
  } catch (e) {
    notify.error(e)
  } finally {
    ready.value = true
  }
  await subscribe(() => events.onSupervisor(onSupervisor))
  await subscribe(() => events.onProfilesChanged(onProfilesChanged))
  await subscribe(() =>
    events.onQuitRequested((active) => {
      quitActive.value = active
      quitOpen.value = true
    }),
  )
  await subscribe(() => events.onApiStatus((status) => settings.setApiStatus(status)))
  await subscribe(() => events.onTasksChanged((event) => automation.applyTasksChanged(event)))
  await subscribe(() => sync.init())
  await subscribe(() => events.onEngineProgress(engines.onProgress))
  // Все подписки готовы: ядро задаёт вопрос «Остановить все профили и выйти?», если Cmd+Q нажали, пока интерфейс грузился.
  core.uiReady().catch((e: unknown) => notify.error(e))
  // Строка Local API в боковом меню; экран «Настройки» перечитывает настройки сам. Загрузка — после подписки:
  // сервер поднимается в фоне (`start_api`), и его `api-status` не теряется между ответом и подпиской.
  subscribed.value = true
})

watch([subscribed, () => security.available], async ([isReady, available]) => {
  if (!isReady || !available || app.info?.init_error) return
  try { await app.load(); await settings.load() } catch (e) { notify.error(e) }
  void engines.startup()
  sync.load().catch((e: unknown) => notify.error(e))
})

onBeforeUnmount(() => unlisten.forEach((stop) => stop()))
</script>

<template>
  <FatalScreen v-if="app.info?.init_error" :error="app.info.init_error" :data-dir="app.info.data_dir" />
  <div v-else-if="ready" class="shell">
    <AppSidebar v-if="!$route.meta.focusedLayout || !security.available" />
    <RouterView v-if="security.available || $route.path === '/settings'" :key="viewKey($route)" />
    <SecurityGate v-else />
  </div>
  <ToastHost />
  <BulkReportDialog v-if="workspaceAvailable" />
  <ConfirmDialog
    v-model:open="quitOpen"
    :title="t('quit.title')"
    :description="t('quit.text', { n: quitActive })"
    :confirm-label="quitting ? t('quit.stopping') : t('quit.confirm')"
    :busy="quitting"
    focus-confirm
    @confirm="quit"
  />
</template>

<style scoped>
.shell {
  display: flex;
  height: 100%;
}
</style>
