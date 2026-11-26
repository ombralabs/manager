<script setup lang="ts">
import { TabsContent, TabsList, TabsRoot, TabsTrigger } from 'reka-ui'
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave, onBeforeRouteUpdate, useRoute, useRouter } from 'vue-router'
import { core } from '../api/core'
import { toCoreError } from '../api/errors'
import type { FieldError, FingerprintOptions, PersonaView } from '../api/types'
import UiButton from '../components/UiButton.vue'
import AppIcon from '../components/AppIcon.vue'
import type { IconName } from '../components/icons'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import { notify } from '../lib/notify'
import {
  type EditorModel,
  type EditorTab,
  clientErrors,
  emptyModel,
  fieldSnapshot,
  mergeFingerprintErrors,
  modelFromDetails,
  tabOfField,
  toProfileForm,
} from '../lib/profileForm'
import { useAppStore } from '../stores/app'
import { useGroupsStore } from '../stores/groups'
import { useProfilesStore } from '../stores/profiles'
import { useClearFieldErrors } from '../lib/useClearFieldErrors'
import { useExtensionsStore } from '../stores/extensions'
import { useProxiesStore } from '../stores/proxies'
import BasicTab from './editor/BasicTab.vue'
import CookiesTab from './editor/CookiesTab.vue'
import FingerprintTab from './editor/FingerprintTab.vue'
import GatewayTab from './editor/GatewayTab.vue'
import HumanizeTab from './editor/HumanizeTab.vue'
import LaunchTab from './editor/LaunchTab.vue'
import SummaryPanel from './editor/SummaryPanel.vue'
import TrafficTab from './editor/TrafficTab.vue'

const route = useRoute()
const router = useRouter()
const { t } = useI18n()
const app = useAppStore()
const groups = useGroupsStore()
const profiles = useProfilesStore()
const proxies = useProxiesStore()
const compactMedia = window.matchMedia('(max-width: 900px)')
const compactNavigation = ref(compactMedia.matches)
const resizeNavigation = () => { compactNavigation.value = compactMedia.matches }

/** Новый профиль попадает в группу, по которой сейчас отфильтрован список. */
function initialGroup(): number | null {
  return typeof profiles.filters.group === 'number' ? profiles.filters.group : null
}

const id = computed(() => typeof route.params.id === 'string' ? route.params.id : null)
const model = ref<EditorModel | null>(null)
const options = ref<FingerprintOptions | null>(null)
const errors = ref<FieldError[]>([])
const fingerprintErrors = ref<FieldError[]>([])
/** «Отпечаток», точка на вкладке и сводка: ошибки формы вместе с живой проверкой. */
const fingerprintTabErrors = computed(() => mergeFingerprintErrors(fingerprintErrors.value, errors.value))
const tab = ref<EditorTab>('basic')
const saving = ref(false)
const regenerating = ref(false)
const content = ref<HTMLElement | null>(null)
const errorSummary = ref<HTMLElement | null>(null)
const original = ref('')
const dirty = computed(() => model.value !== null && JSON.stringify(model.value) !== original.value)
const leaveOpen = ref(false)
let afterLeave: (() => void) | null = null
let cancelLeave: (() => void) | null = null
watch(leaveOpen, (open) => {
  if (!open) {
    cancelLeave?.()
    cancelLeave = null
    afterLeave = null
  }
})
function beforeClose(action: () => void, cancel?: () => void) {
  if (saving.value || regenerating.value) {
    cancel?.()
    return
  }
  if (!dirty.value) {
    action()
    return
  }
  afterLeave = action
  cancelLeave = cancel ?? null
  leaveOpen.value = true
}
function discard() {
  cancelLeave = null
  leaveOpen.value = false
  const action = afterLeave
  afterLeave = null
  action?.()
}
function close() {
  void router.push('/profiles')
}
defineExpose({ beforeClose })
const confirmNavigation = () => new Promise<boolean>(resolve => beforeClose(() => resolve(true), () => resolve(false)))
onBeforeRouteLeave(confirmNavigation)
onBeforeRouteUpdate(confirmNavigation)
watch(tab, async () => { await nextTick(); content.value?.scrollTo?.({ top: 0 }) })
const tabIcons: Record<EditorTab, IconName> = { basic: 'profiles', gateway: 'proxies', fingerprint: 'fingerprint', launch: 'play', cookies: 'cookie', humanize: 'mouse', traffic: 'journal' }
const navigation = computed(() => [
  { label: 'profile', tabs: ['basic', 'gateway', 'fingerprint', 'launch', 'cookies'] as EditorTab[] },
  { label: 'automation', tabs: ['humanize'] as EditorTab[] },
  ...(id.value ? [{ label: 'statistics', tabs: ['traffic'] as EditorTab[] }] : []),
])
/** Время замены шлюза при последнем запуске; `null` — не заменялся или профиль новый. */
const replacedAt = ref<number | null>(null)
/** Почерк сохранённого профиля; у нового — `null`: seed выдаётся при создании. */
const persona = ref<PersonaView | null>(null)
const title = computed(() => (id.value ? model.value?.name || '…' : t('editor.newTitle')))

/** Статус — из полного списка профилей: `App.vue` обновляет его по событиям супервизора. */
const running = computed(() => {
  const state = id.value ? groups.rows.find((row) => row.id === id.value)?.status.state : undefined
  return state === 'starting' || state === 'waiting' || state === 'running' || state === 'paused'
})

const tabHasErrors = (name: EditorTab) =>
  name === 'fingerprint'
    ? fingerprintTabErrors.value.length > 0
    : errors.value.some((error) => tabOfField(error.field) === name)

async function loadModel(): Promise<EditorModel> {
  if (!id.value) return emptyModel(await core.newFingerprint(), initialGroup())
  const details = await core.getProfile(id.value)
  replacedAt.value = details.profile.gateway_replaced_at
  persona.value = details.persona
  return modelFromDetails(details)
}

let mounted = true
onMounted(async () => {
  compactMedia.addEventListener('change', resizeNavigation)
  // Группы, теги и полный список профилей — для «Основного» и статуса профиля.
  if (!groups.loaded) groups.refresh().catch((e) => notify.error(e))
  // Список прокси для «Шлюза» и сводки: экран «Прокси» перечитывает его сам.
  proxies.refresh().catch((e: unknown) => notify.error(e))
  // Наборы для «Запуска» и сводки: экран «Расширения» перечитывает их сам.
  useExtensionsStore().refresh().catch((e: unknown) => notify.error(e))
  try {
    const [loadedOptions, initial] = await Promise.all([core.fingerprintOptions(), loadModel()])
    if (!mounted) return
    options.value = loadedOptions
    model.value = initial
    original.value = JSON.stringify(initial)
    await nextTick()
    if (!id.value) content.value?.querySelector<HTMLInputElement>('#name')?.focus()
  } catch (e) {
    if (!mounted) return
    notify.error(e)
    void router.push('/profiles')
  }
})

let validateTimer: ReturnType<typeof setTimeout> | undefined
let validateRequest = 0

onBeforeUnmount(() => {
  compactMedia.removeEventListener('change', resizeNavigation)
  mounted = false
  cancelLeave?.()
  clearTimeout(validateTimer)
  validateRequest++
})
watch(
  () => model.value?.fingerprint,
  (fingerprint) => {
    if (!fingerprint) return
    clearTimeout(validateTimer)
    validateTimer = setTimeout(async () => {
      const request = ++validateRequest
      try {
        const result = await core.validateFingerprint(fingerprint)
        if (request === validateRequest) fingerprintErrors.value = result
      } catch {
        // Сводка просто не обновится; ошибку покажет сохранение.
      }
    }, 150)
  },
  { deep: true },
)

async function regenerate() {
  if (!model.value || saving.value || regenerating.value) return
  regenerating.value = true
  try {
    model.value.fingerprint = await core.regenerateFingerprint(model.value.fingerprint)
  } catch (e) {
    notify.error(e)
  } finally {
    regenerating.value = false
  }
}

function showErrors(list: FieldError[]) {
  errors.value = list
  if (list.length > 0) {
    tab.value = tabOfField(list[0].field)
  }
  void nextTick(() => errorSummary.value?.focus({ preventScroll: false }))
  notify.warning(t('editor.fixErrors'))
}

useClearFieldErrors(errors, model, (field) => (model.value ? fieldSnapshot(model.value, field) : ''))

async function save() {
  if (!model.value) return
  const local = clientErrors(model.value)
  if (local.length > 0) {
    showErrors(local)
    return
  }
  saving.value = true
  try {
    const form = toProfileForm(model.value)
    if (id.value) await core.updateProfile(id.value, form)
    else await core.createProfile(form)
    notify.info(t(id.value ? 'editor.saved' : 'editor.created'))
    // Счётчики групп и подсказки тегов; экраны списков сами обновятся при открытии.
    void groups.refresh().catch(() => undefined)
    original.value = JSON.stringify(model.value)
    void router.push('/profiles')
  } catch (e) {
    const err = toCoreError(e)
    if (err.code === 'Validation') showErrors(err.fields)
    else notify.error(err)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <main class="page editor">
    <header class="head">
      <div class="editor-heading">
        <UiButton variant="ghost" size="icon" icon="prev" :aria-label="t('editor.workspace.back')" :disabled="saving || regenerating" @click="close" />
        <div class="title-group">
          <div class="hint">{{ t(id ? 'editor.workspace.editing' : 'editor.workspace.creating') }}</div>
          <h1 class="page-title" :title="title">{{ title }}</h1>
        </div>
      </div>
      <div class="editor-meta">
        <span v-if="model" class="platform-badge">macOS {{ model.fingerprint.os_version }} · {{ model.fingerprint.brand }}</span>
        <span v-if="running" class="running-badge"><span />{{ t('status.running') }}</span>
        <UiButton variant="ghost" size="icon" icon="close" :aria-label="t('editor.drawer.close')" :disabled="saving || regenerating" @click="close" />
      </div>
    </header>

    <TabsRoot v-if="model && options" v-model="tab" :orientation="compactNavigation ? 'horizontal' : 'vertical'" class="editor-workspace">
      <TabsList class="editor-nav" :aria-label="t('editor.tabs.label')">
        <div v-for="group in navigation" :key="group.label" class="nav-group">
          <div class="nav-label">{{ t('editor.workspace.groups.' + group.label) }}</div>
          <TabsTrigger v-for="name in group.tabs" :key="name" :value="name" class="tab" :disabled="saving">
            <AppIcon :name="tabIcons[name]" :size="18" />
            <span>{{ t(`editor.tabs.${name}`) }}</span>
            <span v-if="tabHasErrors(name)" class="dot" aria-hidden="true" />
          </TabsTrigger>
        </div>
      </TabsList>
      <div ref="content" class="editor-main">
        <div class="section-heading">
          <h2>{{ t(`editor.tabs.${tab}`) }}</h2>
          <p>{{ t(`editor.workspace.descriptions.${tab}`) }}</p>
        </div>
        <div v-if="errors.length" ref="errorSummary" class="editor-error-summary" role="alert" tabindex="-1">
          <AppIcon name="alert" :size="18" /><span>{{ t('editor.fixErrors') }}</span>
        </div>
        <fieldset class="editor-fields" :disabled="saving || regenerating">
          <TabsContent value="basic"><BasicTab v-model="model" :errors="errors" :profile-id="id" @open-fingerprint="tab = 'fingerprint'" /></TabsContent>
          <TabsContent value="fingerprint">
            <FingerprintTab
              v-model="model.fingerprint"
              v-model:seedText="model.seedText"
              v-model:latText="model.latText"
              v-model:lonText="model.lonText"
              v-model:quotaText="model.quotaText"
              :options="options"
              :direct="model.connectionMode === 'direct'"
              :errors="fingerprintTabErrors"
              @regenerate="regenerate"
            />
          </TabsContent>
          <TabsContent value="gateway">
            <GatewayTab v-model="model" :profile-id="id" :running="running" :replaced-at="replacedAt" :errors="errors" />
          </TabsContent>
          <TabsContent value="cookies"><CookiesTab :profile-id="id" :profile-name="model.name" :running="running" /></TabsContent>
          <TabsContent value="launch"><LaunchTab v-model="model" :errors="errors" /></TabsContent>
          <TabsContent value="humanize"><HumanizeTab v-model="model" :errors="errors" :profile-id="id" :persona="persona" @reset="persona = $event.persona" /></TabsContent>
          <TabsContent v-if="id" value="traffic"><TrafficTab :profile-id="id" :live="running" /></TabsContent>
        </fieldset>
      </div>
      <div class="editor-summary">
        <SummaryPanel :model="model" :profile-id="id" :errors="fingerprintTabErrors" :engine-version="options.engine_version ?? app.engineVersion" :busy="saving || regenerating" @open-tab="tab = $event" @regenerate="regenerate" />
      </div>
    </TabsRoot>
    <div v-else class="editor-loading" role="status"><AppIcon name="spinner" class="spin" />{{ t('editor.drawer.loading') }}</div>

    <footer class="editor-actions">
      <div class="save-state" role="status"><span :class="['save-dot', { dirty }]" />{{ t(dirty ? 'editor.drawer.unsaved' : id ? 'editor.drawer.saved' : 'editor.workspace.draft') }}</div>
      <div class="buttons">
        <UiButton :disabled="saving || regenerating" @click="close">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="primary" icon="save" :loading="saving" :disabled="!model || regenerating" @click="save">{{ t(id ? 'editor.save' : 'editor.create') }}</UiButton>
      </div>
    </footer>
    <ConfirmDialog v-model:open="leaveOpen" :title="t('editor.drawer.discardTitle')" :description="t('editor.drawer.discardText')" :confirm-label="t('editor.drawer.discard')" @confirm="discard" />
  </main>
</template>

<style scoped>
.editor { flex: 1; height: 100%; min-height: 0; padding: 0; gap: 0; overflow: hidden; background: var(--sidebar); }
.head { display: flex; align-items: center; justify-content: space-between; gap: 24px; flex-shrink: 0; padding: 20px 28px; border-bottom: 1px solid var(--border); background: var(--surface); }
.editor-heading { display: flex; align-items: center; gap: 16px; min-width: 0; }
.title-group { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.page-title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 24px; line-height: 32px; }
.editor-meta { display: flex; align-items: center; gap: 16px; flex-shrink: 0; }
.platform-badge { color: var(--text-2); font-size: 12px; }
.running-badge { display: inline-flex; align-items: center; gap: 6px; color: var(--ok-text); font-size: 12px; }
.running-badge > span { width: 6px; height: 6px; border-radius: 50%; background: var(--ok); }
.editor-workspace { display: grid; grid-template-columns: 176px minmax(0, 1fr) 280px; gap: 24px; flex: 1; min-height: 0; padding: 24px 28px; overflow: hidden; }
.editor-nav { display: flex; flex-direction: column; gap: 24px; overflow-y: auto; padding: 2px; margin: -2px; }
.nav-group { display: flex; flex-direction: column; gap: 4px; }
.nav-label { padding: 0 12px 8px; color: var(--text-2); font-size: 12px; font-weight: 500; }
.tab { display: flex; align-items: center; gap: 10px; width: 100%; min-height: 44px; padding: 8px 12px; border: 1px solid transparent; border-radius: var(--radius-control); background: transparent; color: var(--text-2); text-align: left; font-size: 13px; cursor: pointer; }
.tab > span:first-of-type { flex: 1; }
.tab:hover { background: var(--surface); color: var(--text); }
.tab[data-state='active'] { border-color: var(--border); background: var(--surface); color: var(--accent-ink); font-weight: 500; }
.tab:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
.dot { flex-shrink: 0; width: 6px; height: 6px; border-radius: 50%; background: var(--danger-text); }
.editor-main, .editor-summary { min-width: 0; min-height: 0; overflow-y: auto; scrollbar-width: thin; }
.editor-main { padding: 0 4px 16px; scroll-padding-top: 20px; }
.section-heading { margin-bottom: 20px; }
.section-heading h2 { margin: 0 0 8px; font-size: 20px; font-weight: 600; line-height: 28px; }
.section-heading p { margin: 0; color: var(--text-2); font-size: 13px; line-height: 20px; }
.editor-fields { min-width: 0; margin: 0; padding: 0; border: 0; }
.editor-fields :deep(.section) { padding: 24px; }
.editor-fields :deep([role='tabpanel']:focus-visible) { outline: 2px solid var(--focus); outline-offset: -2px; border-radius: var(--radius-card); }
.editor-error-summary { display: flex; align-items: center; gap: 8px; margin-bottom: 16px; padding: 12px 16px; border: 1px solid var(--danger-border); border-radius: var(--radius-control); background: var(--danger-bg); color: var(--danger-text); font-size: 13px; }
.editor-error-summary:focus { outline: 2px solid var(--focus); outline-offset: 2px; }
.editor-actions { display: flex; align-items: center; justify-content: space-between; flex-shrink: 0; gap: 16px; padding: 16px 28px; border-top: 1px solid var(--border); background: var(--surface); }
.save-state { display: inline-flex; align-items: center; gap: 8px; color: var(--text-2); font-size: 12px; }
.save-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--border-strong); }
.save-dot.dirty { background: var(--accent); }
.buttons { display: flex; gap: 8px; }
.editor-loading { display: flex; align-items: center; justify-content: center; gap: 8px; flex: 1; color: var(--text-2); }
@media (max-width: 1150px) {
  .editor-workspace { grid-template-columns: 156px minmax(0, 1fr) 248px; gap: 16px; padding: 20px; }
  .editor-fields :deep(.grid.two), .editor-fields :deep(.grid.three), .editor-fields :deep(.grid.four) { grid-template-columns: minmax(0, 1fr); }
  .editor-fields :deep(.choose), .editor-fields :deep(.facts) { grid-template-columns: minmax(0, 1fr); }
  .editor-fields :deep(.pinned-head) { flex-wrap: wrap; gap: 16px; }
  .editor-fields :deep(.add) { margin-top: 0; }
  .platform-badge { display: none; }
}
@media (max-width: 900px) {
  .head { padding: 16px; }
  .editor-workspace { display: flex; flex-direction: column; gap: 20px; overflow-y: auto; }
  .editor-nav { flex: 0 0 auto; flex-direction: row; gap: 4px; overflow-x: auto; padding-bottom: 8px; }
  .nav-group { display: contents; }
  .nav-label { display: none; }
  .tab { flex-shrink: 0; width: auto; }
  .editor-main, .editor-summary { flex-shrink: 0; overflow: visible; }
  .editor-actions { padding: 12px 16px; }
  .save-state { max-width: 140px; }
}
</style>
