<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { CookieStats } from '../../api/types'
import ConfirmDialog from '../../components/ConfirmDialog.vue'
import UiButton from '../../components/UiButton.vue'
import { notify } from '../../lib/notify'
import CookieExportDialog from '../profiles/CookieExportDialog.vue'
import CookieImportDialog from './CookieImportDialog.vue'

const props = defineProps<{ profileId: string | null; profileName: string; running: boolean }>()
const { t } = useI18n()

const stats = ref<CookieStats | null>(null)
const importOpen = ref(false)
const exportOpen = ref(false)
const clearOpen = ref(false)
const clearing = ref(false)

const exportIds = computed(() => (props.profileId === null ? [] : [props.profileId]))
const exportNames = computed(() => (props.profileId === null ? {} : { [props.profileId]: props.profileName }))

/** Номер последнего запроса: ответ на более старый отбрасывается. */
let latest = 0

async function loadStats() {
  const id = props.profileId
  if (id === null) return
  const request = ++latest
  try {
    const result = await core.cookieStats(id)
    if (request === latest) stats.value = result
  } catch (e) {
    if (request === latest) notify.error(e)
  }
}

watch(
  () => props.profileId,
  () => {
    stats.value = null
    void loadStats()
  },
  { immediate: true },
)

/** При остановке ядро сохраняет cookies браузера — число меняется. */
watch(
  () => props.running,
  (now, before) => {
    if (before && !now) void loadStats()
  },
)

async function clear() {
  const id = props.profileId
  if (id === null) return
  clearing.value = true
  try {
    await core.clearCookies(id)
    notify.info(t('editor.cookies.cleared'))
    await loadStats()
  } catch (e) {
    notify.error(e)
  } finally {
    clearing.value = false
    clearOpen.value = false
  }
}
</script>

<template>
  <section class="card section">
    <div class="intro">
      <h2 class="section-title">{{ t('editor.cookies.title') }}</h2>
      <div class="hint">{{ t('editor.cookies.text') }}</div>
    </div>
    <div v-if="profileId === null" class="hint">{{ t('editor.cookies.newProfile') }}</div>
    <template v-else>
      <div class="count">{{ stats ? t('editor.cookies.count', { n: stats.total }) : '…' }}</div>
      <div v-if="stats && stats.unreadable > 0" class="notice">
        <div class="notice-title">{{ t('editor.cookies.unreadable', stats.unreadable) }}</div>
        <div>{{ t('editor.cookies.unreadableHint') }}</div>
      </div>
      <div class="buttons">
        <UiButton icon="download" @click="importOpen = true">{{ t('editor.cookies.import') }}</UiButton>
        <UiButton icon="upload" @click="exportOpen = true">{{ t('editor.cookies.export') }}</UiButton>
        <span :title="running ? t('editor.cookies.stopFirst') : undefined">
          <UiButton variant="danger" :disabled="running" @click="clearOpen = true">{{ t('editor.cookies.clear') }}</UiButton>
        </span>
      </div>
      <div v-if="running" class="hint">{{ t('editor.cookies.stopFirst') }}</div>
      <CookieImportDialog v-model:open="importOpen" :profile-id="profileId" @imported="loadStats" />
      <CookieExportDialog v-model:open="exportOpen" :ids="exportIds" :names="exportNames" />
      <ConfirmDialog
        v-model:open="clearOpen"
        :title="t('editor.cookies.clearTitle', { name: profileName })"
        :description="t('editor.cookies.clearText')"
        :confirm-label="t('editor.cookies.clearConfirm')"
        :busy="clearing"
        danger
        @confirm="clear"
      />
    </template>
  </section>
</template>

<style scoped>
.intro {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.count {
  font-weight: 600;
}

.notice {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.notice-title {
  font-weight: 600;
}

.buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
}
</style>
