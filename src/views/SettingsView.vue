<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import AppIcon from '../components/AppIcon.vue'
import SettingsSaveStatus from '../components/SettingsSaveStatus.vue'
import { notify } from '../lib/notify'
import { PREF_SETTINGS_SECTION, readPref, writePref } from '../lib/prefs'
import { legacySettingsTarget, searchSettings, settingsEntries, settingsSection, settingsSections, type SettingsEntry } from '../lib/settingsNavigation'
import { useSettingsStore } from '../stores/settings'
import { useSecurityStore } from '../stores/security'
import AutomationCard from './settings/AutomationCard.vue'
import ApiCard from './settings/ApiCard.vue'
import DataCard from './settings/DataCard.vue'
import EngineCard from './settings/EngineCard.vue'
import GeneralCard from './settings/GeneralCard.vue'
import ProfilesCard from './settings/ProfilesCard.vue'
import SecurityCard from './settings/SecurityCard.vue'

const { t } = useI18n()
const settings = useSettingsStore()
const security = useSecurityStore()
const route = useRoute()
const router = useRouter()
const remembered = ref(readPref(PREF_SETTINGS_SECTION, settingsSection, 'general'))
const current = computed(() => settingsSection(route.hash.slice(1)) ?? remembered.value)
const page = ref<HTMLElement | null>(null)
const query = ref('')
const results = computed(() => searchSettings(query.value, t))
const highlighted = ref<string | null>(null)
const loadError = ref(false)
const loading = ref(false)

watch(() => route.hash, (hash) => {
  const legacy = legacySettingsTarget(hash)
  if (legacy) { void router.replace(legacy); return }
  const section = settingsSection(hash.slice(1))
  if (section) { remembered.value = section; writePref(PREF_SETTINGS_SECTION, section) }
}, { immediate: true })

watch(current, () => {
  if (page.value) page.value.scrollTop = 0
  query.value = ''
  highlighted.value = null
})

/** Restrict URL targets to known controls; focus waits for async settings to render. */
async function focusSetting() {
  if (query.value.trim()) return
  const entry = settingsEntries.find((entry) => entry.target === route.query.focus && entry.section === current.value)
  if (!entry) return
  await nextTick()
  const element = page.value?.querySelector<HTMLElement>(`#${entry.target}`)
  if (!element) return
  if (!element.matches('input, button, select, textarea, [tabindex]')) element.tabIndex = -1
  element.focus({ preventScroll: true })
  element.scrollIntoView?.({ block: 'center' })
  highlighted.value = entry.target
}
watch([() => route.fullPath, () => settings.loaded, query, loading], () => { void focusSetting() }, { flush: 'post' })

async function choose(entry: SettingsEntry) {
  query.value = ''
  await router.push({ path: '/settings', hash: '#' + entry.section, query: { focus: entry.target } })
  await focusSetting()
}
async function load() {
  loading.value = true
  loadError.value = false
  try { await settings.load() } catch (error) { loadError.value = true; notify.error(error) }
  finally { loading.value = false; void focusSetting() }
}
onMounted(() => { if (security.available) void load() })
</script>

<template>
  <main ref="page" class="page settings-page" :data-highlight="highlighted">
    <header class="head"><h1 class="page-title">{{ t('settings.title') }}</h1></header>
    <SecurityCard v-if="!security.available" />
    <template v-else>
      <div class="search">
        <AppIcon name="search" :size="18" />
        <label for="settings-search" class="sr-only">{{ t('settings.search.label') }}</label>
        <input id="settings-search" v-model="query" type="search" autocomplete="off" :placeholder="t('settings.search.placeholder')" aria-controls="settings-content" @keydown.esc="query = ''" />
      </div>
      <nav class="section-nav" :aria-label="t('settings.sectionsNav')">
        <RouterLink v-for="id in settingsSections" :key="id" class="section-link" :class="{ active: id === current }"
          :to="{ path: '/settings', hash: '#' + id }" :aria-current="id === current ? 'page' : undefined" @click="query = ''">
          {{ t('settings.sections.' + id) }}
        </RouterLink>
      </nav>
      <div id="settings-content" class="column">
        <template v-if="query.trim()">
          <p class="hint search-count" role="status">{{ results.length ? t('settings.search.count', { n: results.length }) : t('settings.search.empty') }}</p>
          <ul v-if="results.length" class="results">
            <li v-for="entry in results" :key="entry.target">
              <button class="result" type="button" @click="choose(entry)">
                <span class="result-section">{{ t('settings.sections.' + entry.section) }}</span>
                <span class="result-name">{{ t(entry.label) }}</span>
                <span class="hint">{{ t(entry.description) }}</span>
              </button>
            </li>
          </ul>
        </template>
        <template v-else>
          <div v-if="loadError" class="notice" role="alert">
            {{ t('settings.loadError') }} <button type="button" class="retry" @click="load">{{ t('settings.retry') }}</button>
          </div>
          <p v-else-if="loading && !settings.loaded" class="hint" role="status">{{ t('settings.loading') }}</p>
          <SettingsSaveStatus v-if="current !== 'security'" :scope="current" />
          <GeneralCard v-if="current === 'general'" />
          <ProfilesCard v-else-if="current === 'profiles'" />
          <template v-else-if="current === 'security'">
            <SecurityCard />
            <DataCard />
          </template>
          <EngineCard v-else-if="current === 'engine'" />
          <template v-else><ApiCard /><AutomationCard /></template>
        </template>
      </div>
    </template>
  </main>
</template>

<style scoped>
.settings-page { gap: 20px; }
.head { display: flex; align-items: center; min-height: 44px; }
.search { display: flex; align-items: center; gap: 10px; width: 100%; max-width: 800px; padding: 0 14px; border: 1px solid var(--border-strong); border-radius: var(--radius-control); color: var(--text-2); }
.search:focus-within { border-color: var(--focus); outline: 2px solid var(--focus-ring); }
.search input { width: 100%; min-width: 0; min-height: 44px; padding: 8px 0; border: 0; outline: 0; background: transparent; }
.search input::placeholder { color: var(--text-2); }
.section-nav { display: flex; flex-wrap: wrap; gap: 4px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
.section-link { display: flex; align-items: center; min-height: 44px; padding: 10px 14px; border-radius: var(--radius-control); color: var(--sidebar-text); text-decoration: none; }
.section-link:hover { background: var(--sidebar-hover); }
.section-link.active { background: var(--sidebar-active); color: var(--accent-ink); font-weight: 500; }
.section-link:focus-visible, .result:focus-visible, .retry:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
.column { display: flex; flex-direction: column; gap: 16px; width: 100%; max-width: 800px; min-width: 0; }
.column > .section { padding: 24px; }
.results { list-style: none; margin: 0; padding: 0; }
.result { display: flex; flex-direction: column; gap: 4px; width: 100%; padding: 16px 12px; border: 0; border-bottom: 1px solid var(--border); background: transparent; text-align: left; cursor: pointer; }
.result:hover { background: var(--sidebar-hover); }
.result-section { font-size: 12px; color: var(--accent-ink); }
.result-name { font-weight: 500; }
.search-count { margin: 0; }
.retry { border: 0; padding: 8px; background: transparent; text-decoration: underline; cursor: pointer; }
/* A focused search result stays visible until the next navigation, including non-input destinations. */
.settings-page[data-highlight] :deep(:focus) { outline: 2px solid var(--focus); outline-offset: 4px; }
@media (max-width: 700px) {
  .settings-page { padding: 20px 16px; }
  .column > .section { padding: 16px; }
  .section-link { padding-inline: 10px; }
}
</style>
