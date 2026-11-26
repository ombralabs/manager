<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import UiSegmented from '../../../../components/UiSegmented.vue'
import VarText from './VarText.vue'

/** Целое, `${…}` или интервал `{min, max}`. `optional`: пустое поле убирает значение. */
const props = defineProps<{ value: unknown; vars: string[]; id?: string; optional?: boolean; invalid?: boolean; rangeOnly?: boolean }>()
const emit = defineEmits<{ update: [value: unknown] }>()
const { t } = useI18n()

type Part = number | string | undefined
const isRange = (v: unknown): v is { min: Part; max: Part } => typeof v === 'object' && v !== null
const mode = ref<'n' | 'r'>(props.rangeOnly || isRange(props.value) ? 'r' : 'n')
const single = ref<Part>(isRange(props.value) ? undefined : (props.value as Part))
const lo = ref<Part>(isRange(props.value) ? props.value.min : props.rangeOnly ? props.value as Part : undefined)
const hi = ref<Part>(isRange(props.value) ? props.value.max : props.rangeOnly ? props.value as Part : undefined)

watch(() => props.value, (v) => {
  if (isRange(v)) { mode.value = 'r'; lo.value = v.min; hi.value = v.max }
  else if (props.rangeOnly) { mode.value = 'r'; lo.value = hi.value = v as Part }
  else if (v !== undefined) { mode.value = 'n'; single.value = v as Part }
  else single.value = lo.value = hi.value = undefined
})
/** Неполное значение наружу не уходит. */
function push() {
  if (mode.value === 'n') {
    if (single.value !== undefined || props.optional) emit('update', single.value)
  } else if (lo.value !== undefined && hi.value !== undefined) emit('update', { min: lo.value, max: hi.value })
}
function setMode(next: 'n' | 'r') {
  if (next === mode.value) return
  mode.value = next
  if (next === 'r') { hi.value = lo.value = single.value } else single.value = lo.value
  if (next === 'r' && lo.value === undefined) emit('update', undefined)
  else push()
}
const set = (which: 's' | 'l' | 'h', v: Part) => { (which === 's' ? single : which === 'l' ? lo : hi).value = v; push() }
</script>

<template>
  <div class="range">
    <UiSegmented v-if="!rangeOnly" :model-value="mode" :ariaLabel="t('automation.editor.w.number')" :options="[{ value: 'n', label: t('automation.editor.w.number') }, { value: 'r', label: t('automation.editor.w.range') }]"
      @update:model-value="setMode" />
    <VarText v-if="mode === 'n'" :id="id" :model-value="single" :vars="vars" numeric :required="!optional" :invalid="invalid" @update:model-value="set('s', $event)" />
    <div v-else class="pair">
      <VarText :id="id" :model-value="lo" :vars="vars" numeric required :invalid="invalid" :aria-label="t('automation.editor.w.from')" @update:model-value="set('l', $event)" />
      <span>–</span>
      <VarText :model-value="hi" :vars="vars" numeric required :invalid="invalid" :aria-label="t('automation.editor.w.to')" @update:model-value="set('h', $event)" />
    </div>
  </div>
</template>

<style scoped>
.range { display: flex; flex-direction: column; gap: 6px; }
.pair { display: flex; align-items: center; gap: 6px; }
.pair > span:not(.var-text) { color: var(--text-2); }
.pair .var-text { flex: 1; }
</style>
