<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FieldInfo } from '../../../api/types'
import UiButton from '../../../components/UiButton.vue'
import UiField from '../../../components/UiField.vue'
import UiSelect from '../../../components/UiSelect.vue'
import UiSwitch from '../../../components/UiSwitch.vue'
import InputsField from './fields/InputsField.vue'
import IntRangeField from './fields/IntRangeField.vue'
import ListField from './fields/ListField.vue'
import MouseField from './fields/MouseField.vue'
import RowsField from './fields/RowsField.vue'
import TargetField from './fields/TargetField.vue'
import VarInsert from './VarInsert.vue'

const props = defineProps<{ field: FieldInfo; value: unknown; error?: string | null; warning?: string | null; vars: string[]; flows: { id: number; name: string; variables?: { name: string }[] }[]; config?: Record<string, unknown>; op?: string }>()
/** `undefined` убирает поле из конфига. */
const emit = defineEmits<{ update: [value: unknown] }>()
const { t, te } = useI18n()

const id = computed(() => `f-${props.field.name}`)
const label = computed(() => {
  const key = `automation.fields.${props.field.name}`
  return (te(key) ? t(key) : props.field.name) + (props.field.required ? ' *' : '')
})
/** Подпись варианта: своя у поля (`top` прокрутки — не направление), общая, иначе значение как есть. */
function choice(v: string) {
  for (const key of [`automation.choices.${props.field.name}.${v}`, `automation.choices.${v}`]) if (te(key)) return t(key)
  return v
}
const kind = computed(() => props.field.kind)
const TEXTLIKE = ['text', 'secret', 'code', 'int', 'var_out', 'var_in', 'any']
const insertable = computed(() => ['text', 'code', 'int', 'any'].includes(kind.value))
const simple = computed(() => TEXTLIKE.includes(kind.value) || ['bool', 'choice', 'process'].includes(kind.value))

const shown = computed(() => props.value ?? props.field.default)
const text = computed(() => {
  const v = props.value
  if (v === undefined || v === null) return ''
  return typeof v === 'string' ? v : typeof v === 'object' ? JSON.stringify(v) : String(v)
})

function fromText(raw: string): unknown {
  if (raw === '') return undefined
  // Число — только если запись канонична: промежуточный текст («1.», «007») остаётся строкой, поле его не переписывает.
  if (kind.value === 'int' && /^-?\d+$/.test(raw) && String(Number(raw)) === raw) return Number(raw)
  if (kind.value === 'any' && /^\s*[[{]/.test(raw)) {
    try { return JSON.parse(raw) } catch { /* остаётся строкой */ }
  }
  return raw
}

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)
const pretty = (v: unknown) => (v === undefined ? '' : JSON.stringify(v, null, 2))

// Поле JSON — для видов, которых форма не знает: неверный текст — ошибка поля, значение не меняется.
const jsonDraft = ref(pretty(props.value))
const jsonBad = ref(false)
/** `bad` — текст не JSON; пустой — значения нет. */
function readJson(raw: string): { bad: boolean; value?: unknown } {
  if (raw.trim() === '') return { bad: false, value: undefined }
  try { return { bad: false, value: JSON.parse(raw) as unknown } } catch { return { bad: true } }
}
function normalize() {
  if (!jsonBad.value) jsonDraft.value = pretty(props.value)
}
// Вид any: пока поле в фокусе, видно набранное — разобранный JSON не переписывает его (курсор не прыгает); на blur — как в документе.
const anyDraft = ref<string | null>(null)
function onText(raw: string) {
  if (kind.value === 'any') anyDraft.value = raw
  emit('update', fromText(raw))
}
function onBlur(e: Event) {
  anyDraft.value = null
  if (kind.value === 'int') {
    const raw = (e.target as HTMLInputElement).value
    if (/^-?\d+$/.test(raw)) emit('update', Number(raw))
  }
}
// Значение сменилось извне (отмена, повтор) — черновик заново и в фокусе, если набранное уже не говорит то же самое.
// Правка другого поля клонирует документ: то же содержимое новым объектом — не смена, неверный черновик остаётся.
watch(() => props.value, (v, old) => {
  if (same(v, old)) return
  if (anyDraft.value !== null && !same(fromText(anyDraft.value), v)) anyDraft.value = null
  const json = readJson(jsonDraft.value)
  if (json.bad || !same(json.value, v)) {
    jsonBad.value = false
    jsonDraft.value = pretty(v)
  }
})
function onJson(raw: string) {
  jsonDraft.value = raw
  const json = readJson(raw)
  jsonBad.value = json.bad
  if (!json.bad) emit('update', json.value)
}
const emptyChoice = computed(() => kind.value === 'choice_list' && Array.isArray(props.value) && props.value.length === 0)
/** Ошибка набранного, которое в процесс не попало (неверный тег, имя переменной с элементом). */
const ownError = ref<string | null>(null)
const shownError = computed(() => (jsonBad.value ? t('automation.editor.expectedJson') : ownError.value ?? (emptyChoice.value ? t('automation.editor.w.pickOne') : props.error)))
/** Переменные Start выбранного подпроцесса — подсказка имён для `inputs`. */
const calleeVars = computed(() => props.flows.find((f) => f.id === Number(props.config?.process))?.variables?.map((v) => v.name) ?? [])

const el = ref<HTMLInputElement | HTMLTextAreaElement | null>(null)
function insert(name: string) {
  const input = el.value
  const token = '${' + name + '}'
  const current = anyDraft.value ?? text.value
  const from = input?.selectionStart ?? current.length
  const to = input?.selectionEnd ?? from
  anyDraft.value = null
  emit('update', fromText(current.slice(0, from) + token + current.slice(to)))
}

const choices = computed(() => props.field.options.map((o) => ({ value: o, label: choice(o) })))
/** Необязательный выбор снимается пунктом «По умолчанию (…)»: поля нет — действует умолчание операции, оно и названо. */
const choiceItems = computed(() => {
  if (props.field.required) return choices.value
  const def = typeof props.field.default === 'string' ? props.field.default : null
  const label = def === null ? t('automation.editor.choiceDefault') : t('automation.editor.choiceDefaultIs', { label: choice(def) })
  return [{ value: '', label }, ...choices.value]
})
const processes = computed(() => [{ value: '', label: t('automation.editor.noProcess') }, ...props.flows.map((f) => ({ value: String(f.id), label: f.name }))])
</script>

<template>
  <UiField :label="label" :input-id="id" :error="shownError" :hint="warning ?? (field.name === 'image_path' ? t('automation.editor.imagePathHint') : undefined)">
    <template v-if="insertable" #badge><VarInsert :vars="vars" @pick="insert" /></template>
    <UiSwitch v-if="kind === 'bool'" :model-value="shown === true" :label="label" @update:model-value="emit('update', $event)" />
    <UiSelect v-else-if="kind === 'choice'" :id="id" :model-value="value == null ? '' : String(value)" :options="choiceItems" :invalid="!!error"
      @update:model-value="emit('update', $event === '' ? undefined : $event)" />
    <UiSelect v-else-if="kind === 'process'" :id="id" :model-value="value == null ? '' : String(value)" :options="processes" :invalid="!!error"
      @update:model-value="emit('update', $event === '' ? undefined : Number($event))" />
    <textarea v-else-if="kind === 'code'" :id="id" ref="el" class="input mono" rows="6" :value="text" :class="{ invalid: !!error }"
      @input="emit('update', fromText(($event.target as HTMLTextAreaElement).value))" />
    <template v-else-if="simple">
      <input :id="id" ref="el" class="input" :class="{ invalid: !!error, mono: kind === 'var_out' || kind === 'var_in' }" :type="kind === 'secret' ? 'password' : 'text'"
        :autocomplete="kind === 'secret' ? 'new-password' : 'off'" :list="kind === 'var_in' ? `${id}-vars` : undefined" :value="anyDraft ?? text"
        @input="onText(($event.target as HTMLInputElement).value)" @blur="onBlur" />
      <div v-if="kind === 'secret'" class="hint">{{ t('automation.editor.passwordHint') }}</div>
      <UiButton v-if="op === 'data.email' && field.name === 'extract'" size="sm" data-preset="digits6" @click="emit('update', '\\b\\d{6}\\b')">{{ t('automation.editor.sixDigits') }}</UiButton>
      <datalist v-if="kind === 'var_in'" :id="`${id}-vars`"><option v-for="name in vars" :key="name" :value="name" /></datalist>
    </template>
    <TargetField v-else-if="kind === 'target' || kind === 'targets'" :id="id" :value="value" :vars="vars" :multi="kind === 'targets'" :invalid="!!shownError"
      @update="emit('update', $event)" @error="ownError = $event" />
    <IntRangeField v-else-if="kind === 'int_range'" :id="id" :value="value" :vars="vars" :optional="!field.required" :invalid="!!shownError" @update="emit('update', $event)" />
    <ListField v-else-if="['choice_list', 'var_in_list', 'js_var_list', 'text_list'].includes(kind)" :id="id" :kind="kind" :value="value" :fallback="field.default"
      :options="choices" :vars="vars" :invalid="!!shownError" @update="emit('update', $event)" @error="ownError = $event" />
    <RowsField v-else-if="kind === 'var_rows'" :id="id" :value="value" :vars="vars" @update="emit('update', $event)" />
    <InputsField v-else-if="kind === 'inputs'" :id="id" :value="value" :vars="vars" :names="calleeVars" @update="emit('update', $event)" />
    <MouseField v-else-if="kind === 'mouse'" :value="value" :vars="vars" :label="label" @update="emit('update', $event)" />
    <textarea v-else :id="id" class="input mono" rows="4" :value="jsonDraft" :class="{ invalid: !!shownError }"
      @input="onJson(($event.target as HTMLTextAreaElement).value)" @blur="normalize" />
  </UiField>
</template>
