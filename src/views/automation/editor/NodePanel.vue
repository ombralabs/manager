<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../../api/core'
import type { Issue, NodePolicy } from '../../../api/types'
import UiButton from '../../../components/UiButton.vue'
import UiField from '../../../components/UiField.vue'
import UiSelect from '../../../components/UiSelect.vue'
import UiSwitch from '../../../components/UiSwitch.vue'
import { linearSequences, useFlowClipboard } from '../../../lib/flowClip'
import { fieldShown, levelOf, opLabel, setConfig, testable, updateNode, variablesFor } from '../../../lib/flowEdit'
import { useAutomationStore } from '../../../stores/automation'
import { useFlowEditorStore } from '../../../stores/flowEditor'
import FieldControl from './FieldControl.vue'
import SelectionPanel from './SelectionPanel.vue'
import ScrollFields from './ScrollFields.vue'
import StartVariables from './StartVariables.vue'

defineProps<{ embedded?: boolean }>()

const { t, te } = useI18n()
const store = useFlowEditorStore()
const automation = useAutomationStore()
onMounted(() => { if (!automation.flowsLoaded) void automation.refreshFlows().catch(() => {}) })

const clip = useFlowClipboard()
const node = computed(() => (store.doc ? levelOf(store.doc, store.path).nodes.find((n) => n.id === store.selected) : undefined))
const op = computed(() => store.catalog.ops.find((o) => o.id === node.value?.op))
const sequence = computed(() => store.doc && node.value ? linearSequences(store.doc, store.path,
  new Map(store.catalog.ops.map((op) => [op.id, op]))).find((ids) => ids.includes(node.value!.id)) : undefined)
const vars = computed(() => (store.doc ? variablesFor(store.doc, store.catalog) : []))
const flows = computed(() => automation.flows.filter((f) => f.id !== store.id))
/** Поля формы: только нужные выбранному варианту операции (вид капчи, вывод OpenAI, операция таблицы). */
const fields = computed(() => (op.value ? op.value.fields.filter((f) => fieldShown(op.value!, f.name, node.value?.config)) : []))
const own = (list: Issue[]) => (node.value ? list.filter((i) => i.node === node.value!.id) : [])
/** Ошибка скрытого поля — у узла: иначе запуск не идёт, а причины не видно. */
const ofField = (i: Issue) => !!i.field && fields.value.some((f) => f.name === i.field)
const errors = computed(() => own(store.report.errors))
/** Предупреждения не мешают запуску — показаны приглушённо, в счётчик ошибок не входят. */
const warnings = computed(() => own(store.report.warnings))
const nodeIssues = computed(() => errors.value.filter((i) => !ofField(i)))
const nodeWarnings = computed(() => warnings.value.filter((i) => !ofField(i)))
/** У узла — с подписью поля, если оно скрыто вариантом: иначе непонятно, какое поле править. */
function issueText(i: Issue) {
  if (!i.field || !op.value?.fields.some((f) => f.name === i.field)) return i.message
  const key = `automation.fields.${i.field}`
  return `${te(key) ? t(key) : i.field}: ${i.message}`
}
const errorOf = (name: string) => errors.value.find((i) => i.field === name)?.message ?? null
const warningOf = (name: string) => warnings.value.find((i) => i.field === name)?.message ?? null
/** Ошибки без узла — имя, переменные Start, цикл, подпроцесс — видны в панели Start. */
const flowIssues = computed(() => (node.value?.op === 'start' ? store.report.errors.filter((i) => i.node === null) : []))
const flowIssueText = (i: Issue) => (i.field?.startsWith('variables.') ? `${i.field.slice('variables.'.length)}: ${i.message}` : i.message)

const title = computed(() => (node.value ? opLabel(node.value.op, op.value?.adspower, te, t) : ''))
const policies = computed(() => [
  { value: '', label: t('automation.editor.policyDefault') },
  { value: 'interrupt', label: t('automation.editor.policyInterrupt') },
  { value: 'skip', label: t('automation.editor.policySkip') },
  { value: 'branch', label: t('automation.editor.policyBranch') },
])

/** «Проверить»: результат или ошибка узла под кнопкой; при смене узла сбрасывается. */
const probe = ref<{ busy: boolean; ok?: string; error?: string }>({ busy: false })
let probeSeq = 0
/** Смена узла или правка его полей делает результат устаревшим; ответ на прошлый запрос не принимается. */
watch(() => `${node.value?.id}:${JSON.stringify(node.value?.config ?? {})}`, () => { probeSeq++; probe.value = { busy: false } })
async function check() {
  if (!store.doc || !node.value) return
  const id = node.value.id
  const seq = ++probeSeq
  probe.value = { busy: true }
  try {
    const ok = await core.testNode(store.doc, id)
    if (seq === probeSeq) probe.value = { busy: false, ok }
  } catch (e) {
    if (seq === probeSeq) probe.value = { busy: false, error: e instanceof Error ? e.message : String(e) }
  }
}

function patch(p: Parameters<typeof updateNode>[3], key?: string) {
  const id = node.value!.id
  store.apply((doc) => updateNode(doc, store.path, id, p), key)
}
function field(name: string, value: unknown) {
  const id = node.value!.id
  store.apply((doc) => setConfig(doc, store.path, id, name, value), `config:${id}:${name}`)
}
</script>

<template>
  <SelectionPanel v-if="store.selection.length > 1" />
  <div v-else-if="!node" class="empty">
    {{ t('automation.editor.panelEmpty') }}
    <UiButton v-if="clip" size="sm" class="paste" @click="store.paste()">{{ t('automation.editor.paste', { n: clip.nodes.length }) }}</UiButton>
  </div>
  <div v-else class="panel" :class="{ embedded }" :data-node="node.id">
    <h2 v-if="!embedded" class="title" tabindex="-1">{{ title }}</h2>
    <section v-if="flowIssues.length > 0" class="flow-issues">
      <h3 class="section-title">{{ t('automation.editor.flowErrors') }}</h3>
      <p v-for="(issue, i) in flowIssues" :key="i" class="error-text">{{ flowIssueText(issue) }}</p>
    </section>
    <p v-for="(issue, i) in nodeIssues" :key="i" class="error-text">{{ issueText(issue) }}</p>
    <p v-for="(issue, i) in nodeWarnings" :key="`w${i}`" class="hint">{{ issueText(issue) }}</p>
    <StartVariables v-if="node.op === 'start'" />
    <p v-if="node.op === 'web.input' || node.op === 'proc.vars'" class="hint">{{ t('automation.editor.passwordHint') }}</p>
    <ScrollFields v-if="node.op === 'web.scroll'" :key="node.id" :fields="fields" :config="node.config" :vars="vars" :errors="errors" :warnings="warnings" @update="field" />
    <template v-else>
      <FieldControl v-for="f in fields" :key="`${node.id}:${f.name}`" :field="f" :value="node.config?.[f.name]" :error="errorOf(f.name)"
        :warning="warningOf(f.name)" :vars="vars" :flows="flows" :config="node.config" :op="node.op" @update="field(f.name, $event)" />
    </template>
    <div v-if="testable(node)" class="probe">
      <UiButton size="sm" :disabled="probe.busy" @click="check">{{ probe.busy ? t('automation.editor.probeBusy') : t('automation.editor.probe') }}</UiButton>
      <p v-if="probe.ok !== undefined" class="probe-ok" data-probe="ok">{{ probe.ok }}</p>
      <p v-if="probe.error !== undefined" class="error-text" data-probe="error">{{ probe.error }}</p>
    </div>
    <UiField :label="t('automation.editor.panelDescription')" input-id="node-description">
      <input id="node-description" class="input" :value="node.description ?? ''" :placeholder="t('automation.editor.panelOptional')"
        @input="patch({ description: ($event.target as HTMLInputElement).value || undefined }, `desc:${node.id}`)" />
    </UiField>
    <details :key="node.id" class="node-options" :open="!!(node.on_error || node.disabled) || undefined">
      <summary>{{ t('automation.editor.panelAdvanced') }}</summary>
      <div class="options-body">
        <UiField :label="t('automation.editor.panelOnError')" input-id="node-on-error">
          <UiSelect id="node-on-error" :model-value="node.on_error ?? ''" :options="policies"
            @update:model-value="patch({ on_error: ($event || undefined) as NodePolicy | undefined })" />
        </UiField>
        <div class="row">
          <span>{{ t('automation.editor.panelDisabled') }}</span>
          <UiSwitch :model-value="node.disabled === true" :label="t('automation.editor.panelDisabled')"
            @update:model-value="patch({ disabled: $event || undefined })" />
        </div>
        <div v-if="node.op !== 'start'" class="copy-row">
          <UiButton size="sm" @click="store.copy([node.id])">{{ t('automation.editor.copy') }}</UiButton>
          <UiButton v-if="node.op === 'control.group'" size="sm" @click="store.ungroup(node.id)">{{ t('automation.editor.ungroup') }}</UiButton>
          <UiButton v-else-if="sequence" size="sm" icon="groupNodes" @click="store.group(sequence)">{{ t('automation.editor.groupSequence') }}</UiButton>
        </div>
      </div>
    </details>
  </div>
</template>

<style scoped>
.empty { color: var(--text-2); font-size: 13px; padding: 16px; }
.panel { display: flex; flex-direction: column; gap: 12px; height: 100%; overflow: auto; padding: 16px; background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius-card); }
.panel.embedded { height: auto; gap: 24px; overflow: visible; padding: 0; border: 0; border-radius: 0; background: transparent; }
.probe { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; }
.probe-ok { margin: 0; font-size: 13px; word-break: break-all; }
.copy-row { display: flex; gap: 8px; flex-wrap: wrap; }
.paste { display: block; margin-top: 12px; }
.title { margin: 0; font-size: 16px; font-weight: 600; }
.row { display: flex; align-items: center; justify-content: space-between; font-size: 13px; }
.error-text, .hint { margin: 0; }
.flow-issues { display: flex; flex-direction: column; gap: 4px; }
.flow-issues .section-title { margin: 0; font-size: 14px; }
.node-options > summary { width: fit-content; color: var(--text-2); font-size: 13px; cursor: pointer; }
.node-options > summary:focus-visible { outline: 2px solid var(--focus); outline-offset: 4px; border-radius: 2px; }
.options-body { display: flex; flex-direction: column; gap: 16px; padding-top: 16px; }
</style>
