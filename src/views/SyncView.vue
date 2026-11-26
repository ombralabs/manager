<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import type { ProfileRow, SyncWindow } from '../api/types'
import UiButton from '../components/UiButton.vue'
import SyncStartDialog from '../components/SyncStartDialog.vue'
import UiCheckbox from '../components/UiCheckbox.vue'
import UiSelect from '../components/UiSelect.vue'
import { notify } from '../lib/notify'
import { useGroupsStore } from '../stores/groups'
import { useSyncStore } from '../stores/sync'

const { t, te } = useI18n()
const router = useRouter()
const sync = useSyncStore()
const groups = useGroupsStore()

const group = ref('all')
const groupOptions = computed(() => [
  { value: 'all', label: t('sync.groupAll') },
  { value: 'none', label: t('sync.groupNone') },
  ...groups.groups.map((g) => ({ value: String(g.id), label: g.name })),
])
const isAlive = (state: string) => state === 'running' || state === 'paused'
const aliveIds = computed(() => new Set(groups.rows.filter((row) => isAlive(row.status.state)).map((row) => row.id)))
// Остановленный профиль не остаётся отмеченным или главным (во время сессии выбор принадлежит сессии).
watch(aliveIds, (ids) => {
  if (!sync.state.active) sync.prune(ids)
})
/** Работающие профили (запущенные и приостановленные) и окна текущей сессии, даже если профиль уже остановлен. */
const profiles = computed(() =>
  groups.rows.filter((row) => {
    if (!isAlive(row.status.state) && !(sync.state.active && windowOf(row.id))) return false
    if (group.value === 'all') return true
    return group.value === 'none' ? row.group === null : String(row.group?.id) === group.value
  }),
)

const active = computed(() => sync.state.active)
const mainId = computed(() => (active.value ? sync.state.main : sync.mainId))
const windowOf = (id: string): SyncWindow | undefined => sync.state.windows.find((w) => w.profileId === id)
const canStart = computed(() => sync.checked.size >= 2 && sync.mainId !== null)
const syncOpen = ref(false)
const syncProfiles = ref<ProfileRow[]>([])
function start() {
  syncProfiles.value = groups.rows.filter((row) => sync.checked.has(row.id))
  syncOpen.value = true
}
const modeLabel = computed(() => t(`sync.mode.${active.value ? sync.state.mode : sync.settings.mode}`))

/** Текст причины: ядро шлёт код и готовую подробность; неизвестный код показывается подробностью как есть. */
const reason = (code: string, detail = '') => (te(`sync.reason.${code}`) ? t(`sync.reason.${code}`, { detail }) : detail || code)
function statusText(id: string): string | null {
  const status = windowOf(id)?.status
  if (id === mainId.value) return t('sync.status.main')
  if (!status) return null
  if (status.state === 'synced') return t('sync.status.synced')
  if (status.state === 'paused') return t('sync.status.paused', { reason: reason(status.code, status.detail) })
  return t('sync.status.dropped', { reason: reason(status.code) })
}

const guard = (action: () => Promise<unknown>) => () => action().catch((e: unknown) => notify.error(e))

onMounted(() => {
  groups.refresh().catch((e: unknown) => notify.error(e))
})
</script>

<template>
  <main class="page list-page">
    <header class="head">
      <div class="title-group">
        <h1 class="page-title">{{ t('sync.title') }}</h1>
        <span class="mode">{{ modeLabel }}</span>
      </div>
      <div class="actions">
        <UiButton @click="router.push('/sync/settings')">{{ t('sync.settings') }}</UiButton>
        <UiButton v-if="active" variant="danger" @click="guard(sync.stop)()">{{ t('sync.stop') }}</UiButton>
        <UiButton v-else variant="primary" :disabled="!canStart || syncOpen" @click="start">{{ t('sync.start') }}</UiButton>
      </div>
    </header>
    <div class="filter">
      <UiSelect v-model="group" :options="groupOptions" :aria-label="t('sync.group')" />
    </div>
    <section class="card list" :aria-label="t('sync.title')">
      <div v-if="profiles.length === 0" class="empty">
        <div class="section-title">{{ t('sync.empty') }}</div>
        <div class="hint">{{ t('sync.emptyHint') }}</div>
      </div>
      <div v-for="row in profiles" :key="row.id" class="item">
        <UiCheckbox
          :model-value="sync.checked.has(row.id)"
          :ariaLabel="row.name"
          :disabled="active"
          @update:model-value="sync.toggle(row.id)"
        />
        <div class="name">
          <div>{{ row.name }}</div>
          <div v-if="windowOf(row.id)?.plain" class="hint">{{ t('sync.plain') }}</div>
        </div>
        <span v-if="statusText(row.id)" :class="['pill', windowOf(row.id)?.status.state, { main: row.id === mainId }]">{{ statusText(row.id) }}</span>
        <div class="buttons">
          <UiButton v-if="!active && sync.checked.has(row.id) && row.id !== mainId" size="sm" @click="sync.makeMain(row.id)">{{ t('sync.makeMain') }}</UiButton>
          <template v-if="windowOf(row.id)?.status.state === 'paused'">
            <UiButton size="sm" @click="guard(() => sync.openLikeMain(row.id))()">{{ t('sync.openLikeMain') }}</UiButton>
            <UiButton size="sm" @click="guard(() => sync.resume(row.id))()">{{ t('sync.resume') }}</UiButton>
          </template>
          <UiButton v-else-if="windowOf(row.id)?.status.state === 'synced' && row.id !== mainId" size="sm" @click="guard(() => sync.release(row.id))()">{{ t('sync.release') }}</UiButton>
          <UiButton size="sm" @click="guard(() => sync.show(row.id))()">{{ t('sync.show') }}</UiButton>
        </div>
      </div>
    </section>
    <SyncStartDialog v-model:open="syncOpen" :profiles="syncProfiles" />
  </main>
</template>

<style scoped>
.mode {
  padding: 3px 10px;
  border-radius: 999px;
  background: var(--muted-bg);
  color: var(--text-2);
  font-size: 12.5px;
  font-weight: 600;
}

.actions,
.buttons {
  display: flex;
  gap: 8px;
}

.filter {
  width: 260px;
}

.list {
  display: flex;
  flex-direction: column;
  min-height: 0;
  overflow: auto;
}

.empty {
  padding: 28px 20px;
}

.item {
  display: flex;
  align-items: center;
  gap: 14px;
  min-height: 56px;
  padding: 0 20px;
  border-bottom: 1px solid var(--row-border);
}

.item:last-child {
  border-bottom: 0;
}

.name {
  flex: 1 1 0;
  min-width: 0;
  font-weight: 500;
}

.pill {
  padding: 3px 10px;
  border-radius: 999px;
  background: var(--ok-bg);
  color: var(--ok-text);
  font-size: 12.5px;
  font-weight: 600;
}

.pill.main {
  background: var(--info-bg);
  color: var(--info-text);
}

.pill.paused {
  background: var(--warn-bg);
  color: var(--warn-text);
}

.pill.dropped {
  background: var(--danger-bg);
  color: var(--danger-text);
}
</style>
