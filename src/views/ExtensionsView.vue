<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import { events } from '../api/core'
import UiButton from '../components/UiButton.vue'
import { notify } from '../lib/notify'
import { useExtensionsStore } from '../stores/extensions'
import AllTab from './extensions/AllTab.vue'
import SetsTab from './extensions/SetsTab.vue'

const { t } = useI18n()
const store = useExtensionsStore()
const route = useRoute()

/** Вкладки — два маршрута одного экрана (холст «Расширения» и «Расширения: наборы»). */
const onSets = computed(() => route.path === '/extensions/sets')
/** Наборы кроме 0: его роль на экране — «включено». */
const sets = computed(() => store.categories.filter((set) => set.id !== 0))
const enabledCount = computed(() => store.extensions.filter((ext) => store.enabledIds.has(ext.id)).length)
const withSet = computed(() => sets.value.reduce((sum, set) => sum + set.profile_count, 0))
const subtitle = computed(() =>
  onSets.value
    ? `${t('extensions.setsCount', sets.value.length)} · ${t('extensions.profilesWithSet', withSet.value)}`
    : `${t('extensions.extensionsCount', store.extensions.length)} · ${t('extensions.enabledCount', { n: enabledCount.value })}`,
)
const allTab = ref<InstanceType<typeof AllTab> | null>(null)
const setsTab = ref<InstanceType<typeof SetsTab> | null>(null)

let unlisten: (() => void) | null = null
let gone = false
onMounted(async () => {
  store.refresh().catch(notify.error)
  // Число профилей в наборах меняют назначения, корзина и Local API.
  try {
    const stop = await events.onProfilesChanged(() => store.scheduleRefresh())
    if (gone) stop()
    else unlisten = stop
  } catch (e) {
    notify.error(e)
  }
})
onUnmounted(() => {
  gone = true
  unlisten?.()
  store.cancelRefresh()
})
</script>

<template>
  <main class="page list-page extensions-page">
    <header class="head">
      <div class="title-group">
        <h1 class="page-title">{{ t('extensions.title') }}</h1>
        <div class="hint">{{ subtitle }}</div>
      </div>
      <UiButton v-if="onSets" variant="primary" icon="plus" @click="setsTab?.create()">{{ t('extensions.newSet') }}</UiButton>
      <UiButton v-else variant="primary" icon="plus" @click="allTab?.add()">{{ t('extensions.addExtension') }}</UiButton>
    </header>
    <nav class="tabs" :aria-label="t('extensions.tabs')">
      <RouterLink to="/extensions" class="tab">{{ t('extensions.tabAll') }}<span class="count">{{ store.extensions.length }}</span></RouterLink>
      <RouterLink to="/extensions/sets" class="tab">{{ t('extensions.sets') }}<span class="count">{{ sets.length }}</span></RouterLink>
    </nav>
    <SetsTab v-if="onSets" ref="setsTab" />
    <AllTab v-else ref="allTab" />
  </main>
</template>

<style scoped>
.extensions-page {
  overflow: hidden;
}

/* Вкладки холста: подчёркнута текущая (`aria-current` ставит RouterLink на точном совпадении пути). */
.tabs {
  display: flex;
  gap: 28px;
  flex-shrink: 0;
  height: 44px;
  border-bottom: 1px solid var(--border);
}

.tab {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  height: 44px;
  margin-bottom: -1px;
  border-bottom: 2px solid transparent;
  color: var(--text-2);
  font-weight: 500;
  text-decoration: none;
  white-space: nowrap;
}

.tab[aria-current='page'] {
  border-bottom-color: var(--text);
  color: var(--text);
  font-weight: 600;
}

.tab:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}

.count {
  padding: 0 8px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 6%, var(--bg));
  color: var(--text-2);
  font-size: 12px;
  line-height: 20px;
  font-weight: 600;
}
</style>
