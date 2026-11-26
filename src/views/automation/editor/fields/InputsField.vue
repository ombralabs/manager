<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import UiButton from '../../../../components/UiButton.vue'
import { VAR_NAME } from '../../../../lib/flowEdit'
import VarText from './VarText.vue'

/** `control.call` → `inputs`: «переменная подпроцесса → значение». Имена подсказаны переменными Start выбранного процесса. */
const props = defineProps<{ value: unknown; vars: string[]; id: string; names: string[] }>()
const emit = defineEmits<{ update: [value: unknown] }>()
const { t } = useI18n()

/** `empty`: строка пришла из процесса с явной пустой строкой — это не «не задано», её пустота пишется. */
interface Row { name: string; value: string; empty?: boolean }
const fromValue = (v: unknown): Row[] =>
  typeof v === 'object' && v !== null ? Object.entries(v).map(([name, x]) => ({ name, value: String(x), empty: x === '' })) : []
const rows = ref<Row[]>(fromValue(props.value))

/**
 * Строки без допустимого или с повторным именем остаются в форме, в процесс не попадают; новая строка с пустым
 * значением — тоже: действует умолчание подпроцесса. Пустая строка, уже записанная в процессе, остаётся.
 */
function toValue(list: Row[]) {
  // Без прототипа: имена вроде `constructor` и `__proto__` — обычные ключи, не унаследованные свойства.
  const out: Record<string, string> = Object.create(null)
  for (const r of list) if (VAR_NAME.test(r.name) && (r.value !== '' || r.empty) && !(r.name in out)) out[r.name] = r.value
  return Object.keys(out).length === 0 ? undefined : out
}
watch(() => props.value, (v) => {
  if (JSON.stringify(v) !== JSON.stringify(toValue(rows.value))) rows.value = fromValue(v)
})
function set(next: Row[]) { rows.value = next; emit('update', toValue(next)) }
const patch = (i: number, p: Partial<Row>) => set(rows.value.map((r, j) => (j === i ? { ...r, ...p } : r)))
</script>

<template>
  <div class="rows">
    <div v-for="(r, i) in rows" :key="i" class="row">
      <input :id="i === 0 ? id : undefined" class="input mono" :value="r.name" :list="`${id}-names`" :placeholder="t('automation.editor.w.inputName')"
        :aria-label="t('automation.editor.w.inputName')" autocomplete="off" @input="patch(i, { name: ($event.target as HTMLInputElement).value })" />
      <VarText :model-value="r.value" :vars="vars" :aria-label="t('automation.editor.w.rowValue')" @update:model-value="patch(i, { value: String($event ?? '') })" />
      <UiButton size="icon" variant="ghost" icon="close" class="del-row" :aria-label="t('automation.editor.w.rowDelete')" @click="set(rows.filter((_, j) => j !== i))" />
    </div>
    <datalist :id="`${id}-names`"><option v-for="name in names" :key="name" :value="name" /></datalist>
    <UiButton class="add-row" size="sm" icon="plus" @click="set([...rows, { name: '', value: '' }])">{{ t('automation.editor.w.rowAdd') }}</UiButton>
  </div>
</template>

<style scoped>
.rows { display: flex; flex-direction: column; gap: 8px; }
.row { display: grid; grid-template-columns: 1fr 1fr auto; gap: 6px; align-items: center; }
.add-row { align-self: flex-start; }
</style>
