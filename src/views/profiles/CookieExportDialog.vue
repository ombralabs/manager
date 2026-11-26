<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { pickFolder, pickSaveFile } from '../../api/dialogs'
import type { CookieFormat, ExportTarget } from '../../api/types'
import type { SelectOption } from '../../components/types'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import UiSegmented from '../../components/UiSegmented.vue'
import { showBulkReport } from '../../lib/bulkReport'
import { cookieExtension, exportFileName } from '../../lib/cookieExport'
import { notify } from '../../lib/notify'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ ids: string[]; names: Record<string, string> }>()
const { t } = useI18n()

const format = ref<CookieFormat>('json')
const exporting = ref(false)
const formatOptions = computed<SelectOption<CookieFormat>[]>(() => [
  { value: 'json', label: t('bulk.export.json') },
  { value: 'netscape', label: t('bulk.export.netscape') },
])

const single = computed(() => props.ids.length === 1)
const firstName = computed(() => {
  const id = props.ids[0]
  return id === undefined ? '' : (props.names[id] ?? id)
})
const hint = computed(() =>
  single.value
    ? t('bulk.export.fileHint', { file: exportFileName(firstName.value, format.value) })
    : t('bulk.export.folderHint', { n: props.ids.length, ext: cookieExtension(format.value) }),
)

/** Нечитаемые записи выбранных профилей: их нет в файле экспорта. Считаются в фоне, по одному профилю. */
const unreadable = ref(0)
/** Номер подсчёта: закрытие диалога или новый набор профилей прерывает прежний. */
let statsRequest = 0

watch(
  [open, () => props.ids],
  async ([isOpen, ids]) => {
    const request = ++statsRequest
    unreadable.value = 0
    if (!isOpen) return
    for (const id of ids) {
      try {
        const stats = await core.cookieStats(id)
        if (request !== statsRequest) return
        unreadable.value += stats.unreadable
      } catch {
        // Счёт — только предупреждение: ошибку профиля покажет отчёт экспорта.
        if (request !== statsRequest) return
      }
    }
  },
  { immediate: true },
)
// Размонтирование прерывает подсчёт: оставшиеся профили не опрашиваются.
onBeforeUnmount(() => {
  statsRequest++
})

/** Один профиль — окно сохранения файла, несколько — выбор папки. Файлы пишет ядро; отмена окна ничего не делает. */
async function run() {
  exporting.value = true
  try {
    let target: ExportTarget
    if (single.value) {
      const path = await pickSaveFile(exportFileName(firstName.value, format.value), format.value)
      if (path === null) return
      target = { kind: 'file', path }
    } else {
      const path = await pickFolder()
      if (path === null) return
      target = { kind: 'folder', path }
    }
    const report = await core.exportCookiesTo(props.ids, format.value, target)
    open.value = false
    showBulkReport('export', report)
  } catch (e) {
    notify.error(e)
  } finally {
    exporting.value = false
  }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('bulk.export.title')" :width="480" :busy="exporting">
    <div class="body">
      <UiField :label="t('bulk.export.format')">
        <UiSegmented v-model="format" :options="formatOptions" :aria-label="t('bulk.export.format')" />
      </UiField>
      <div class="hint">{{ hint }}</div>
      <div v-if="unreadable > 0" class="notice">{{ t('bulk.export.unreadable', unreadable) }}</div>
    </div>
    <template #footer>
      <UiButton :disabled="exporting" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" icon="upload" :loading="exporting" @click="run">{{ t('bulk.export.submit') }}</UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.body {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
</style>
