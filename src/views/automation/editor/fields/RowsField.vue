<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import UiButton from '../../../../components/UiButton.vue'
import UiSelect from '../../../../components/UiSelect.vue'
import VarText from './VarText.vue'

/** `proc.vars`: строки «Имя / Тип / Значение». Пустое имя ядро отклонит — это видно у поля. */
const props = defineProps<{ value: unknown; vars: string[]; id: string }>()
const emit = defineEmits<{ update: [value: unknown] }>()
const { t } = useI18n()

interface Row { name: string; type: string; value: string }
const TYPES = ['string', 'number', 'boolean', 'array', 'object', 'text', 'file', 'folder']
const types = TYPES.map((v) => ({ value: v, label: v }))
const rows = () => (Array.isArray(props.value) ? (props.value as Row[]) : [])
function put(next: Row[]) { emit('update', next.length === 0 ? undefined : next) }
const patch = (i: number, p: Partial<Row>) => put(rows().map((r, j) => (j === i ? { ...r, ...p } : r)))
const add = () => put([...rows(), { name: '', type: 'string', value: '' }])
const remove = (i: number) => put(rows().filter((_, j) => j !== i))
const text = (v: unknown) => (typeof v === 'string' ? v : v === undefined || v === null ? '' : typeof v === 'object' ? JSON.stringify(v) : String(v))
</script>

<template>
  <div class="rows">
    <div v-for="(r, i) in rows()" :key="i" class="row">
      <input :id="i === 0 ? id : undefined" class="input mono" :value="r.name" :placeholder="t('automation.editor.w.rowName')" :aria-label="t('automation.editor.w.rowName')"
        autocomplete="off" @input="patch(i, { name: ($event.target as HTMLInputElement).value })" />
      <UiSelect :model-value="r.type" :options="types" :ariaLabel="t('automation.editor.w.rowType')" @update:model-value="patch(i, { type: $event })" />
      <VarText :model-value="text(r.value)" :vars="vars" :aria-label="t('automation.editor.w.rowValue')" @update:model-value="patch(i, { value: String($event ?? '') })" />
      <UiButton size="icon" variant="ghost" icon="close" class="del-row" :aria-label="t('automation.editor.w.rowDelete')" @click="remove(i)" />
    </div>
    <UiButton class="add-row" size="sm" icon="plus" @click="add">{{ t('automation.editor.w.rowAdd') }}</UiButton>
  </div>
</template>

<style scoped>
.rows { display: flex; flex-direction: column; gap: 8px; }
.row { display: grid; grid-template-columns: 1fr 110px 1fr auto; gap: 6px; align-items: center; }
.add-row { align-self: flex-start; }
</style>
