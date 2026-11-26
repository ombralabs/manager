<script setup lang="ts">
import { reactive, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import UiField from '../../../../components/UiField.vue'
import UiSelect from '../../../../components/UiSelect.vue'
import UiSwitch from '../../../../components/UiSwitch.vue'
import { VAR_NAME } from '../../../../lib/flowEdit'
import IntRangeField from './IntRangeField.vue'
import VarText from './VarText.vue'

/** `target` и `targets`: селектор или (только у `target`) элемент из переменной. */
const props = defineProps<{ value: unknown; vars: string[]; id: string; multi?: boolean; invalid?: boolean }>()
const emit = defineEmits<{ update: [value: unknown]; error: [message: string | null] }>()
const { t } = useI18n()

type Draft = { element: boolean; name: string; by: string; selector: string; index: unknown; frame: string }
const read = (v: unknown): Draft => {
  const o = (typeof v === 'object' && v !== null ? v : {}) as Record<string, unknown>
  return {
    element: typeof o.element === 'string',
    name: typeof o.element === 'string' ? o.element.replace(/^\$\{(.*)\}$/, '$1') : '',
    by: typeof o.by === 'string' ? o.by : 'css',
    selector: typeof o.selector === 'string' ? o.selector : '',
    index: o.index,
    frame: typeof o.frame === 'string' ? o.frame : '',
  }
}
const d = reactive(read(props.value))

/** Пока селектор (или имя переменной) пуст, значения нет: ядро такую цель отклонило бы. */
function build(): unknown {
  if (d.element) return d.name === '' ? undefined : { element: '${' + d.name + '}' }
  if (d.selector === '') return undefined
  const out: Record<string, unknown> = { by: d.by, selector: d.selector }
  if (!props.multi && d.index !== undefined) out.index = d.index
  if (d.frame !== '') out.frame = d.frame
  return out
}
// Правка другого поля клонирует документ: то же содержимое новым объектом — не смена, незаписанное имя остаётся.
watch(() => props.value, (v, old) => {
  if (JSON.stringify(v) === JSON.stringify(old) || JSON.stringify(v) === JSON.stringify(build())) return
  Object.assign(d, read(v))
  emit('error', null)
})
function change(patch: Partial<Draft>) {
  Object.assign(d, patch)
  // Имя переменной с элементом проверяется как у var_in: неверное видно ошибкой поля, цель в процессе — прежняя.
  const bad = d.element && d.name !== '' && !VAR_NAME.test(d.name)
  emit('error', bad ? t('automation.editor.w.badName', { name: d.name }) : null)
  if (!bad) emit('update', build())
}
const by = () => ['css', 'xpath', 'text'].map((v) => ({ value: v, label: t(`automation.editor.w.${v}`) }))
</script>

<template>
  <div class="target">
    <div v-if="!multi" class="sw">
      <span>{{ t('automation.editor.w.fromVar') }}</span>
      <UiSwitch :model-value="d.element" :label="t('automation.editor.w.fromVar')" @update:model-value="change({ element: $event })" />
    </div>
    <template v-if="d.element">
      <input :id="id" class="input mono" :class="{ invalid }" :value="d.name" :list="`${id}-vars`" :aria-label="t('automation.editor.w.elementVar')"
        autocomplete="off" @input="change({ name: ($event.target as HTMLInputElement).value.trim() })" />
      <datalist :id="`${id}-vars`"><option v-for="name in vars" :key="name" :value="name" /></datalist>
    </template>
    <template v-else>
      <UiField :label="t('automation.editor.w.by')">
        <UiSelect :model-value="d.by" :options="by()" @update:model-value="change({ by: $event })" />
      </UiField>
      <UiField :label="t('automation.editor.w.selector')" :input-id="id">
        <VarText :id="id" :model-value="d.selector" :vars="vars" mono :invalid="invalid" @update:model-value="change({ selector: String($event ?? '') })" />
      </UiField>
      <UiField v-if="!multi" :label="t('automation.editor.w.index')">
        <IntRangeField :value="d.index" :vars="vars" optional @update="change({ index: $event })" />
      </UiField>
      <UiField :label="t('automation.editor.w.frame')">
        <VarText :model-value="d.frame" :vars="vars" mono :aria-label="t('automation.editor.w.frame')" @update:model-value="change({ frame: String($event ?? '') })" />
      </UiField>
    </template>
  </div>
</template>

<style scoped>
.target { display: flex; flex-direction: column; gap: 10px; }
.sw { display: flex; align-items: center; justify-content: space-between; font-size: 13px; }
</style>
