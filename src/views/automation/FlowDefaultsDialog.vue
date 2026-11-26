<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FlowRow } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import { notify } from '../../lib/notify'
import { formDefault, parseStrictNumber } from '../../lib/paramDefault'
import { useAutomationStore } from '../../stores/automation'
import ParamField from './ParamField.vue'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ flow: FlowRow | null }>()
const { t } = useI18n()
const automation = useAutomationStore()
const values = ref<Record<string, string | boolean>>({})
/** Что показано при открытии: сравнением находим тронутые поля. */
let initial: Record<string, string | boolean> = {}
const errors = ref<Record<string, string>>({})
const saving = ref(false)

watch(
  open,
  (isOpen) => {
    if (!isOpen || !props.flow) return
    errors.value = {}
    initial = Object.fromEntries(props.flow.variables.map((v) => [v.name, formDefault(v)]))
    values.value = { ...initial }
  },
  { immediate: true },
)

/** Уходят только тронутые поля; очищенное — `null`: ядро ставит пустое значение типа. */
function build(): Record<string, unknown> | null {
  const out: Record<string, unknown> = {}
  const bad: Record<string, string> = {}
  for (const v of props.flow?.variables ?? []) {
    const raw = values.value[v.name]
    if (raw === initial[v.name]) continue
    if (typeof raw === 'boolean') {
      out[v.name] = raw
      continue
    }
    const text = String(raw ?? '')
    if (text.trim() === '') {
      out[v.name] = null
      continue
    }
    if (v.type === 'number') {
      const n = parseStrictNumber(text)
      if (n === null) bad[v.name] = t('automation.taskDialog.expectNumber')
      else out[v.name] = n
    } else if (v.type === 'array' || v.type === 'object') {
      try {
        out[v.name] = JSON.parse(text)
      } catch {
        bad[v.name] = t('automation.taskDialog.expectJson')
      }
    } else out[v.name] = text
  }
  errors.value = bad
  return Object.keys(bad).length > 0 ? null : out
}

async function submit() {
  const flow = props.flow
  const params = build()
  if (!flow || !params) return
  saving.value = true
  try {
    await automation.setDefaults(flow.id, params)
    open.value = false
  } catch (e) {
    notify.error(e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('automation.defaultsDialog.title', { name: flow?.name ?? '' })" :width="560" :busy="saving">
    <p v-if="!flow?.variables.length" class="hint">{{ t('automation.defaultsDialog.empty') }}</p>
    <ParamField v-for="v in flow?.variables ?? []" :key="v.name" v-model="values[v.name]!" :decl="v" :error="errors[v.name]" />
    <template #footer>
      <UiButton :disabled="saving" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" :loading="saving" @click="submit">{{ t('common.save') }}</UiButton>
    </template>
  </UiDialog>
</template>
