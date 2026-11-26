<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FlowDoc, FlowSettings } from '../../../api/types'
import BrowserModeField from '../../../components/BrowserModeField.vue'
import UiButton from '../../../components/UiButton.vue'
import UiDialog from '../../../components/UiDialog.vue'
import UiField from '../../../components/UiField.vue'
import UiSelect from '../../../components/UiSelect.vue'
import UiSwitch from '../../../components/UiSwitch.vue'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ doc: FlowDoc }>()
const emit = defineEmits<{ apply: [change: (doc: FlowDoc) => FlowDoc] }>()
const { t } = useI18n()

const onError = ref<'interrupt' | 'skip'>('interrupt')
const afterTabs = ref<'clear' | 'keep'>('clear')
const afterBrowser = ref<'quit' | 'keep'>('quit')
const headless = ref<boolean | null>(false)
const timeout = ref('')
const delay = ref('')
const args = ref('')
const clearCache = ref(false)
const group = ref('')
const remark = ref('')

watch(open, (value) => {
  if (!value) return
  const s = props.doc.settings ?? {}
  onError.value = s.on_error ?? 'interrupt'
  afterTabs.value = s.after_tabs ?? 'clear'
  afterBrowser.value = s.after_browser ?? 'quit'
  headless.value = s.headless ?? false
  timeout.value = s.timeout_s === undefined ? '' : String(s.timeout_s)
  delay.value = s.delay_min === undefined ? '' : String(s.delay_min)
  args.value = (s.launch_args ?? []).join('\n')
  clearCache.value = s.clear_cache_on_quit ?? false
  group.value = props.doc.group ?? ''
  remark.value = props.doc.remark ?? ''
}, { immediate: true })

// Тайм-аут и задержка в ядре — u64: только целые от 0, иначе ядро не прочтёт весь документ.
const number = (text: string) => (text.trim() === '' ? undefined : Number(text.trim()))
const wrong = (text: string) => (text.trim() !== '' && !/^\d+$/.test(text.trim()) ? t('automation.editor.s.wholeNumber') : null)
const timeoutError = computed(() => wrong(timeout.value))
const delayError = computed(() => wrong(delay.value))

function apply() {
  if (timeoutError.value || delayError.value) return
  // Пишется только изменённое: «Применить» без изменений оставляет документ как был.
  const old = props.doc.settings ?? {}
  const settings: FlowSettings = { ...old }
  const put = <K extends keyof FlowSettings>(key: K, value: FlowSettings[K], fallback?: FlowSettings[K]) => {
    if (JSON.stringify(value) === JSON.stringify(old[key] ?? fallback)) return
    if (value === undefined) delete settings[key]
    else settings[key] = value
  }
  put('on_error', onError.value, 'interrupt')
  put('after_tabs', afterTabs.value, 'clear')
  put('after_browser', afterBrowser.value, 'quit')
  put('headless', headless.value ?? false, false)
  put('timeout_s', number(timeout.value))
  put('delay_min', number(delay.value))
  put('launch_args', args.value.split('\n').map((l) => l.trim()).filter(Boolean), [])
  put('clear_cache_on_quit', clearCache.value, false)
  const g = group.value.trim()
  const r = remark.value.trim()
  emit('apply', (doc) => {
    const next: FlowDoc = { ...doc }
    if (doc.settings || Object.keys(settings).length > 0) next.settings = settings
    if (g !== (doc.group ?? '').trim()) {
      if (g) next.group = g
      else delete next.group
    }
    if (r !== (doc.remark ?? '').trim()) {
      if (r) next.remark = r
      else delete next.remark
    }
    return next
  })
  open.value = false
}

const opts = <T extends string>(...values: T[]) => values.map((value) => ({ value, label: t(`automation.editor.s.${value}`) }))
</script>

<template>
  <UiDialog v-model:open="open" :title="t('automation.editor.settings')" :width="560">
    <div class="form">
      <BrowserModeField v-model="headless" />
      <UiField :label="t('automation.editor.s.onError')">
        <UiSelect v-model="onError" :options="opts('interrupt', 'skip')" :aria-label="t('automation.editor.s.onError')" />
      </UiField>
      <UiField :label="t('automation.editor.s.afterTabs')" :hint="afterTabs === 'clear' ? t('automation.editor.s.clearHint') : undefined">
        <UiSelect v-model="afterTabs" :options="opts('clear', 'keep')" :aria-label="t('automation.editor.s.afterTabs')" />
      </UiField>
      <UiField :label="t('automation.editor.s.afterBrowser')">
        <UiSelect v-model="afterBrowser" :options="opts('quit', 'keep')" :aria-label="t('automation.editor.s.afterBrowser')" />
      </UiField>
      <div class="grid two">
        <UiField :label="t('automation.editor.s.timeout')" input-id="flow-timeout" :error="timeoutError">
          <input id="flow-timeout" v-model="timeout" class="input" :class="{ invalid: timeoutError }" inputmode="numeric" />
        </UiField>
        <UiField :label="t('automation.editor.s.delay')" input-id="flow-delay" :error="delayError">
          <input id="flow-delay" v-model="delay" class="input" :class="{ invalid: delayError }" inputmode="numeric" />
        </UiField>
      </div>
      <UiField :label="t('automation.editor.s.launchArgs')" input-id="flow-args" :hint="t('automation.editor.s.launchArgsHint')">
        <textarea id="flow-args" v-model="args" class="input mono" rows="3" />
      </UiField>
      <label class="switch-row"><UiSwitch v-model="clearCache" :label="t('automation.editor.s.clearCache')" />{{ t('automation.editor.s.clearCache') }}</label>
      <UiField :label="t('automation.editor.s.group')" input-id="flow-group">
        <input id="flow-group" v-model="group" class="input" />
      </UiField>
      <UiField :label="t('automation.editor.s.remark')" input-id="flow-remark">
        <textarea id="flow-remark" v-model="remark" class="input" rows="2" />
      </UiField>
    </div>
    <template #footer>
      <UiButton variant="primary" @click="apply">{{ t('automation.editor.apply') }}</UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.form { display: flex; flex-direction: column; gap: 14px; }
.switch-row { display: flex; align-items: center; gap: 12px; }
</style>
