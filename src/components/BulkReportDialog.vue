<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { useProfilesStore } from '../stores/profiles'
import { currentReport } from '../lib/bulkReport'
import { errorTitle } from '../lib/errorText'
import UiButton from './UiButton.vue'
import UiDialog from './UiDialog.vue'

const { t } = useI18n()
const router = useRouter()
const profiles = useProfilesStore()

const reason = (code: string, ownerHint?: string) => errorTitle(code, ownerHint)

/** Временный фильтр «эти профили» на экране «Профили». */
async function showInList() {
  const ids = currentReport.value?.lines.map((line) => line.id) ?? []
  currentReport.value = null
  profiles.showOnly(ids)
  await router.push('/profiles')
}
</script>

<template>
  <UiDialog :open="currentReport !== null" @update:open="currentReport = null" :title="currentReport?.title ?? ''">
    <ul class="lines">
      <li v-for="line in currentReport?.lines ?? []" :key="line.id">
        <span class="name">{{ line.name }}</span> — {{ reason(line.code, line.ownerHint) }}
      </li>
    </ul>
    <template #footer>
      <UiButton @click="currentReport = null">{{ t('common.close') }}</UiButton>
      <UiButton v-if="currentReport?.inList" variant="primary" @click="showInList">{{ t('dialog.showInList') }}</UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.lines {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 320px;
  margin: 0;
  padding: 0;
  overflow: auto;
  overflow-wrap: anywhere;
  list-style: none;
  font-size: 13px;
}

.name {
  font-weight: 600;
}
</style>
