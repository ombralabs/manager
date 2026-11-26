<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { pickCookieFile } from '../../api/dialogs'
import type { CookieImportReport, CookieSource } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import { skipText } from '../../lib/errorText'
import { notify } from '../../lib/notify'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ profileId: string }>()
const emit = defineEmits<{ imported: [report: CookieImportReport] }>()
const { t } = useI18n()
/** Причина пропуска — переводом кода: русский текст ядра (`reason`) — только для журнала. */
const skipReason = (code: string) => skipText(code)

const text = ref('')
const busy = ref(false)
/** Открыто окно выбора файла: кнопки неактивны, второе окно не открывается. */
const picking = ref(false)
const report = ref<CookieImportReport | null>(null)

watch(open, (isOpen) => {
  if (!isOpen) return
  text.value = ''
  report.value = null
})

/** Формат (JSON AdsPower или cookies.txt) определяет ядро; файл тоже читает ядро. */
async function run(source: CookieSource) {
  busy.value = true
  try {
    const result = await core.importCookies(props.profileId, source)
    report.value = result
    text.value = ''
    emit('imported', result)
  } catch (e) {
    notify.error(e)
  } finally {
    busy.value = false
  }
}

function importText() {
  if (text.value.trim() !== '') void run({ kind: 'text', text: text.value })
}

/** Отмена окна выбора файла ничего не делает. Пока окно открыто, повторный щелчок его не открывает. */
async function importFile() {
  if (picking.value || busy.value) return
  picking.value = true
  let path: string | null = null
  try {
    path = await pickCookieFile()
  } catch (e) {
    notify.error(e)
  } finally {
    picking.value = false
  }
  if (path !== null) await run({ kind: 'file', path })
}

function again() {
  report.value = null
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('editor.cookies.importTitle')" :width="560" :busy="busy || picking">
    <div v-if="report" class="result">
      <div class="summary">{{ t('editor.cookies.result', { added: report.added, replaced: report.replaced }) }}</div>
      <div v-if="report.sent_to_browser" class="hint">{{ t('editor.cookies.sentToBrowser') }}</div>
      <div v-if="report.skipped.length > 0" class="skipped">
        <div class="hint">{{ t('editor.cookies.skipped', { n: report.skipped.length }) }}</div>
        <ul class="lines">
          <li v-for="item in report.skipped" :key="item.line">
            {{ t('editor.cookies.skippedLine', { line: item.line, reason: skipReason(item.code) }) }}
          </li>
        </ul>
      </div>
    </div>
    <UiField v-else :label="t('editor.cookies.textLabel')" input-id="cookie-import-text" :hint="t('editor.cookies.formatHint')">
      <textarea
        id="cookie-import-text"
        v-model="text"
        class="input mono"
        rows="8"
        spellcheck="false"
        :disabled="busy || picking"
      />
    </UiField>
    <template #footer>
      <template v-if="report">
        <UiButton @click="again">{{ t('editor.cookies.more') }}</UiButton>
        <UiButton variant="primary" @click="open = false">{{ t('editor.cookies.done') }}</UiButton>
      </template>
      <template v-else>
        <UiButton :disabled="busy || picking" @click="open = false">{{ t('common.cancel') }}</UiButton>
        <UiButton icon="folder" :disabled="busy || picking" @click="importFile">{{ t('editor.cookies.fromFile') }}</UiButton>
        <UiButton
          variant="primary"
          icon="download"
          :loading="busy"
          :disabled="text.trim() === '' || picking"
          @click="importText"
        >
          {{ t('editor.cookies.submit') }}
        </UiButton>
      </template>
    </template>
  </UiDialog>
</template>

<style scoped>
.result {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.summary {
  font-weight: 600;
}

.skipped {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.lines {
  max-height: 200px;
  margin: 0;
  padding-left: 18px;
  overflow: auto;
  font-size: 13px;
  line-height: 18px;
}
</style>
