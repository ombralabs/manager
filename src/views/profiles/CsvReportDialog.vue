<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { CsvImportReport } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import { csvLineErrorText } from '../../lib/csvImport'

/** Ошибок в отчёте может быть до тысячи; показываются первые. */
const SHOWN = 50

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ report: CsvImportReport | null }>()
const { t } = useI18n()

const lines = computed(() => (props.report?.errors ?? []).slice(0, SHOWN).map(csvLineErrorText))
const more = computed(() => Math.max(0, (props.report?.errors.length ?? 0) - SHOWN))
</script>

<template>
  <UiDialog v-model:open="open" :title="t('profiles.importMenu.reportTitle')" :width="520">
    <div class="summary">{{ t('profiles.importMenu.created', report?.created.length ?? 0) }}</div>
    <ul class="lines">
      <li v-for="line in lines" :key="line" class="report-line">{{ line }}</li>
    </ul>
    <div v-if="more > 0" class="hint">{{ t('profiles.importMenu.more', { n: more }) }}</div>
    <template #footer>
      <UiButton @click="open = false">{{ t('common.close') }}</UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.summary {
  font-weight: 600;
}

.lines {
  max-height: 320px;
  margin: 0;
  padding: 0;
  overflow: auto;
  list-style: none;
}

.report-line {
  padding: 4px 0;
  border-bottom: 1px solid var(--row-border);
  font-size: 13px;
}
</style>
