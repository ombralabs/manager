<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type { LineError, ProxyKind } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiCheckbox from '../../components/UiCheckbox.vue'
import UiField from '../../components/UiField.vue'
import UiSelect from '../../components/UiSelect.vue'
import { kindOptions } from '../../lib/proxyOptions'
import { notify } from '../../lib/notify'
import { importLineCount, keepErrorLines, lineErrorText } from '../../lib/proxies'
import { useProxiesStore } from '../../stores/proxies'

const { t } = useI18n()
const proxies = useProxiesStore()

const text = ref('')
const kind = ref<ProxyKind>('http')
const checkAfter = ref(true)
const importing = ref(false)
/** Ошибки последнего импорта, номера — по строкам, оставшимся в поле. */
const lineErrors = ref<LineError[]>([])

const count = computed(() => importLineCount(text.value))
/** «Добавлено N» (и повторы); строки с ошибками остаются в поле, остальные убираются. */
async function submit() {
  if (count.value === 0 || importing.value) return
  // Номера строк в отчёте ядра — по отправленному тексту; поле на время импорта заблокировано.
  const sent = text.value
  importing.value = true
  try {
    const report = await proxies.importText(sent, kind.value, checkAfter.value)
    notify.info(
      t('proxies.import.added', report.added),
      report.duplicates > 0 ? t('proxies.import.duplicates', { n: report.duplicates }) : undefined,
    )
    const rest = keepErrorLines(sent, report.errors)
    text.value = rest.text
    lineErrors.value = rest.errors
  } catch (e) {
    notify.error(e)
  } finally {
    importing.value = false
  }
}
</script>

<template>
  <aside class="panel card section" :aria-label="t('proxies.import.title')">
    <h2 class="section-title">{{ t('proxies.import.title') }}</h2>
    <UiField :label="t('proxies.import.lines')" input-id="proxy-import-text">
      <textarea
        id="proxy-import-text"
        v-model="text"
        :disabled="importing"
        class="input mono"
        rows="6"
        spellcheck="false"
        autocomplete="off"
        :placeholder="t('proxies.import.placeholder')"
      />
    </UiField>
    <ul v-if="lineErrors.length > 0" class="line-errors">
      <li v-for="error in lineErrors" :key="error.line" class="error-text">{{ lineErrorText(error) }}</li>
    </ul>
    <div class="hint formats">
      <span>{{ t('proxies.import.formats') }}</span>
      <span class="mono format">{{ t('proxies.import.formatUrl') }}</span>
      <span class="mono format">{{ t('proxies.import.formatColon') }}</span>
    </div>
    <UiField :label="t('proxies.import.defaultKind')" input-id="proxy-import-kind">
      <UiSelect id="proxy-import-kind" v-model="kind" :options="kindOptions" />
    </UiField>
    <label class="check-row">
      <UiCheckbox v-model="checkAfter" :ariaLabel="t('proxies.import.checkAfter')" />
      <span>{{ t('proxies.import.checkAfter') }}</span>
    </label>
    <UiButton variant="primary" class="submit" :loading="importing" :disabled="count === 0" @click="submit">
      {{ t('proxies.import.submit', count) }}
    </UiButton>
  </aside>
</template>

<style scoped>
.panel {
  flex-shrink: 0;
  align-self: flex-start;
  width: 340px;
  max-height: 100%;
  overflow: auto;
}

/* Поле холста: 132 px, моно 12/18, без ручки изменения размера. */
.panel textarea {
  height: 132px;
  font-size: 12px;
  line-height: 18px;
  resize: none;
}

.line-errors {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.formats {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px 12px;
  border-radius: 10px;
  background: var(--bg);
  font-size: 12px;
}

.format {
  font-size: 12px;
  color: var(--text);
}

.check-row {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 13px;
  cursor: pointer;
}

.submit {
  width: 100%;
}
</style>
