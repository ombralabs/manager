<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import type { FlowDoc, VarDecl, VarType } from '../../../api/types'
import UiSelect from '../../../components/UiSelect.vue'
import { useFlowEditorStore } from '../../../stores/flowEditor'

const TYPES: VarType[] = ['string', 'number', 'boolean', 'array', 'object', 'text', 'file', 'folder']
const { t } = useI18n()
const store = useFlowEditorStore()
const rows = () => store.doc?.variables ?? []
const typeOptions = TYPES.map((v) => ({ value: v, label: v }))

function setRows(next: VarDecl[], key?: string) {
  store.apply((doc): FlowDoc => ({ ...doc, variables: next }), key)
}
const patch = (i: number, p: Partial<VarDecl>, key?: string) => setRows(rows().map((r, j) => (j === i ? { ...r, ...p } : r)), key)

const shown = (v: unknown) => (v === null || v === undefined ? '' : typeof v === 'string' ? v : JSON.stringify(v))
function parse(type: VarType, raw: string): unknown {
  if (raw === '') return null
  // Промежуточный текст («1.», «007») остаётся строкой, пока не нормализован на blur.
  if (type === 'number' && raw.trim() !== '' && String(Number(raw)) === raw) return Number(raw)
  if (type === 'boolean' && (raw === 'true' || raw === 'false')) return raw === 'true'
  if (type === 'array' || type === 'object') {
    try { return JSON.parse(raw) } catch { /* остаётся строкой */ }
  }
  return raw
}
/** Ошибки переменной (поле ядра `variables.<имя>`) — у её строки; список «Ошибки процесса» их тоже держит. */
const errorsOf = (name: string) => store.report.errors.filter((i) => i.node === null && i.field === `variables.${name}`).map((i) => i.message)
const add = () => setRows([...rows(), { name: '', type: 'string', default: null, description: '' }])
</script>

<template>
  <section class="vars">
    <h3 class="section-title">{{ t('automation.editor.varsTitle') }}</h3>
    <div v-for="(row, i) in rows()" :key="i" class="var-row">
      <input class="input mono var-name" :class="{ invalid: errorsOf(row.name).length > 0 }" :value="row.name" :placeholder="t('automation.editor.varName')" :aria-label="t('automation.editor.varName')"
        @input="patch(i, { name: ($event.target as HTMLInputElement).value }, `var:${i}:name`)" />
      <UiSelect :model-value="row.type" :options="typeOptions" :aria-label="t('automation.editor.varType')"
        @update:model-value="patch(i, { type: $event, default: parse($event, shown(row.default)) })" />
      <input class="input var-default" :value="shown(row.default)" :placeholder="t('automation.editor.varDefault')" :aria-label="t('automation.editor.varDefault')"
        @input="patch(i, { default: parse(row.type, ($event.target as HTMLInputElement).value) }, `var:${i}:default`)"
        @blur="row.type === 'number' && typeof row.default === 'string' && row.default.trim() !== '' && !Number.isNaN(Number(row.default)) && patch(i, { default: Number(row.default) })" />
      <input class="input" :value="row.description" :placeholder="t('automation.editor.varDescription')" :aria-label="t('automation.editor.varDescription')"
        @input="patch(i, { description: ($event.target as HTMLInputElement).value }, `var:${i}:description`)" />
      <button type="button" class="link del-var" :aria-label="t('automation.editor.varDelete')" @click="setRows(rows().filter((_, j) => j !== i))">×</button>
      <p v-for="(message, j) in errorsOf(row.name)" :key="j" class="error-text var-error">{{ message }}</p>
    </div>
    <button type="button" class="link add-var" @click="add">{{ t('automation.editor.varAdd') }}</button>
    <p class="hint">{{ t('automation.editor.varPasswordHint') }}</p>
  </section>
</template>

<style scoped>
.vars { display: flex; flex-direction: column; gap: 8px; }
.section-title { margin: 0; font-size: 14px; font-weight: 600; }
.var-row { display: grid; grid-template-columns: 1fr 1fr auto; gap: 6px; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius-card); }
.var-row .var-name { grid-column: 1; }
.var-row > :nth-child(3), .var-row > :nth-child(4) { grid-column: 1 / 3; }
.var-row > .del-var { grid-column: 3; grid-row: 1; }
.var-row > .var-error { grid-column: 1 / -1; margin: 0; }
.add-var { align-self: flex-start; }
</style>
