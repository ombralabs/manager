<script setup lang="ts">
import { ref, watch } from 'vue'
import VarInsert from '../VarInsert.vue'

/** Однострочное поле со вставкой `{x}`. `numeric`: целое или `${…}`; промежуточный текст («1.», «007») остаётся черновиком до ухода из поля. */
const props = defineProps<{ modelValue: string | number | undefined; vars: string[]; id?: string; numeric?: boolean; required?: boolean; mono?: boolean; invalid?: boolean; ariaLabel?: string }>()
const emit = defineEmits<{ 'update:modelValue': [value: string | number | undefined] }>()

const show = (v: string | number | undefined) => (v === undefined ? '' : String(v))
const draft = ref(show(props.modelValue))
const el = ref<HTMLInputElement | null>(null)

/** `ok: false` — текст ещё не значение, наружу не уходит. */
function parse(raw: string): { ok: boolean; value?: string | number } {
  if (raw === '') return { ok: !props.numeric || !props.required }
  if (!props.numeric) return { ok: true, value: raw }
  if (/^-?\d+$/.test(raw) && String(Number(raw)) === raw) return { ok: true, value: Number(raw) }
  return raw.includes('${') ? { ok: true, value: raw } : { ok: false }
}
function set(raw: string) {
  draft.value = raw
  const p = parse(raw)
  if (p.ok) emit('update:modelValue', p.value)
}
watch(() => props.modelValue, (v) => {
  const p = parse(draft.value)
  if (!p.ok || p.value !== v) draft.value = show(v)
})
const normalize = () => { draft.value = show(props.modelValue) }
function insert(name: string) {
  const token = '${' + name + '}'
  const from = el.value?.selectionStart ?? draft.value.length
  const to = el.value?.selectionEnd ?? from
  set(draft.value.slice(0, from) + token + draft.value.slice(to))
}
</script>

<template>
  <span class="var-text">
    <input :id="id" ref="el" class="input" :class="{ mono, invalid }" :value="draft" :aria-label="ariaLabel" autocomplete="off"
      @input="set(($event.target as HTMLInputElement).value)" @blur="normalize" />
    <VarInsert :vars="vars" @pick="insert" />
  </span>
</template>

<style scoped>
.var-text { display: flex; align-items: center; gap: 4px; min-width: 0; }
.var-text .input { flex: 1; min-width: 0; }
</style>
