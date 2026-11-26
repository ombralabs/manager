<script setup lang="ts">
import { computed, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FlowNode } from '../../../api/types'
import UiButton from '../../../components/UiButton.vue'
import UiSwitch from '../../../components/UiSwitch.vue'
import { inBrowserBody, removeItems } from '../../../lib/flowEdit'
import { useFlowDebugStore } from '../../../stores/flowDebug'
import { useFlowEditorStore } from '../../../stores/flowEditor'

const props = defineProps<{ node: FlowNode; sequence?: string[] }>()
const { t } = useI18n()
const editor = useFlowEditorStore()
const debug = useFlowDebugStore()
const start = computed(() => props.node.op === 'start')
const group = computed(() => props.node.op === 'control.group')
const chosen = computed(() => editor.selection.includes(props.node.id) && editor.selection.length > 1 ? editor.selection : [props.node.id])
const multiple = computed(() => chosen.value.length > 1)
const canDebug = computed(() => !debug.running && !(editor.doc && inBrowserBody(editor.doc, [...editor.path, props.node.id])))
const groupLabel = computed(() => t(multiple.value ? 'automation.editor.group' : 'automation.editor.groupSequenceCount', { n: props.sequence?.length ?? 0 }))

function select() {
  editor.selection = [props.node.id]
  editor.selected = props.node.id
}
function settings() {
  editor.openNodeSettings(multiple.value ? undefined : props.node.id)
}
async function showDebug() {
  select()
  debug.open = true
  await nextTick()
  document.querySelector<HTMLInputElement>('.debug-panel input')?.focus({ preventScroll: true })
}
function remove() {
  editor.apply((doc) => removeItems(doc, editor.path, chosen.value, []))
}
</script>

<template>
  <div class="node-tools nodrag nopan" role="toolbar" :aria-label="t('automation.editor.nodeActions')"
    @pointerdown.stop @mousedown.stop @click.stop @dblclick.stop
    @keydown.enter.stop @keydown.space.stop @keydown.delete.stop @keydown.backspace.stop>
    <template v-if="!start">
      <UiButton v-if="group && !multiple" size="icon" variant="ghost" icon="ungroupNodes"
        :aria-label="t('automation.editor.ungroup')" :title="t('automation.editor.ungroup')" @click="editor.ungroup(node.id)" />
      <UiButton v-else size="icon" variant="ghost" icon="groupNodes" :disabled="!multiple && !sequence?.length"
        :aria-label="groupLabel" :title="!multiple && !sequence?.length ? t('automation.editor.noSequence') : groupLabel"
        @click="editor.group(multiple ? chosen : sequence)" />
    </template>
    <UiButton size="icon" variant="ghost" icon="play" :disabled="!canDebug"
      :aria-label="t('automation.editor.nodeDebug')" :title="debug.running ? t('automation.editor.debugRunning') : canDebug ? t('automation.editor.nodeDebug') : t('automation.editor.debugFromBody')" @click="showDebug" />
    <UiButton v-if="!start" size="icon" variant="ghost" icon="copy"
      :aria-label="t('automation.editor.duplicate')" :title="t('automation.editor.duplicate')" @click="editor.duplicate(chosen)" />
    <UiButton size="icon" variant="ghost" icon="settings" :aria-label="t('automation.editor.nodeSettings')"
      :title="t('automation.editor.nodeSettings')" @click="settings" />
    <UiButton v-if="!start" size="icon" variant="ghost" icon="trash" class="delete-node"
      :aria-label="t('automation.editor.deleteNodes')" :title="t('automation.editor.deleteNodes')" @click="remove" />
    <template v-if="!start">
      <span class="separator" aria-hidden="true" />
      <UiSwitch :model-value="!node.disabled" :label="t('automation.editor.nodeEnabled')" :title="t('automation.editor.nodeEnabled')"
        @update:model-value="editor.toggleDisabled([node.id], !$event)" />
    </template>
  </div>
</template>

<style scoped>
.node-tools { position: absolute; bottom: calc(100% + 6px); left: 0; display: flex; align-items: center; padding: 1.2px; border: 1.2px solid var(--border-strong); border-radius: 7.2px; background: var(--surface); box-shadow: 0 2.4px 9.6px #17274614; cursor: default; }
.node-tools::after { content: ''; position: absolute; top: 100%; left: 0; right: 0; height: 8px; }
.node-tools .btn.square { width: 24px; height: 24px; border-radius: 4.8px; }
.node-tools :deep(.icon) { width: 14.4px; height: 14.4px; }
.node-tools :deep(.switch) { width: 31.2px; height: 19.2px; padding: 2.4px; }
.node-tools :deep(.thumb) { width: 14.4px; height: 14.4px; }
.node-tools .delete-node:hover { color: var(--danger-text); background: var(--danger-bg); }
.separator { height: 14.4px; width: 1.2px; margin: 0 2.4px; background: var(--border); }
</style>
