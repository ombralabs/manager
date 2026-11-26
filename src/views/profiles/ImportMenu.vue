<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuRoot, DropdownMenuTrigger } from 'reka-ui'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { pickArchiveFile, pickCsvFile } from '../../api/dialogs'
import { toCoreError } from '../../api/errors'
import type { CsvImportReport, ProfileRecord } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import { notify } from '../../lib/notify'
import { useToasts } from '../../stores/toasts'
import ArchivePasswordDialog from './ArchivePasswordDialog.vue'
import CsvReportDialog from './CsvReportDialog.vue'

const emit = defineEmits<{ imported: [] }>()
const { t } = useI18n()

/** Пока идёт импорт, кнопка неактивна: второй импорт поверх первого не нужен. */
const busy = ref(false)
const reportOpen = ref(false)
const report = ref<CsvImportReport | null>(null)
const passwordOpen = ref(false)
const archivePath = ref('')

async function importCsv() {
  busy.value = true
  try {
    const path = await pickCsvFile()
    if (path === null) return
    const result = await core.importCsvFile(path)
    if (result.errors.length > 0) {
      report.value = result
      reportOpen.value = true
    } else if (result.created.length === 0) {
      notify.info(t('profiles.importMenu.csvEmpty'))
    } else {
      notify.info(t('profiles.importMenu.created', result.created.length))
    }
    if (result.created.length > 0) emit('imported')
  } catch (e) {
    const err = toCoreError(e)
    // Таблица без колонки name: ядро отвечает ошибкой проверки поля.
    if (err.code === 'Validation' && err.fields.some((field) => field.field === 'name')) useToasts().push({ kind: 'error', title: t('profiles.importMenu.noNameColumn') })
    else notify.error(err)
  } finally {
    busy.value = false
  }
}

function archiveDone(record: ProfileRecord) {
  notify.info(t('profiles.importMenu.archiveDone', { name: record.name }))
  emit('imported')
}

/** Сначала без пароля: архив без секретов импортируется сразу, с секретами — ядро просит пароль. */
async function importArchive() {
  let picked = ''
  busy.value = true
  try {
    const path = await pickArchiveFile()
    if (path === null) return
    picked = path
    archiveDone(await core.importProfile(path, null))
  } catch (e) {
    const err = toCoreError(e)
    if (err.code === 'ArchivePassword' && picked !== '') {
      archivePath.value = picked
      passwordOpen.value = true
    } else {
      notify.error(err)
    }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <DropdownMenuRoot>
    <DropdownMenuTrigger as-child>
      <UiButton icon="download" :loading="busy">{{ t('profiles.import') }}</UiButton>
    </DropdownMenuTrigger>
    <DropdownMenuPortal>
      <DropdownMenuContent align="end" :side-offset="4" class="popover">
        <DropdownMenuItem class="menu-item" :disabled="busy" @select="importCsv">{{ t('profiles.importMenu.csv') }}</DropdownMenuItem>
        <DropdownMenuItem class="menu-item" :disabled="busy" @select="importArchive">{{ t('profiles.importMenu.archive') }}</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenuPortal>
  </DropdownMenuRoot>
  <CsvReportDialog v-model:open="reportOpen" :report="report" />
  <ArchivePasswordDialog v-model:open="passwordOpen" :path="archivePath" @imported="archiveDone" />
</template>
