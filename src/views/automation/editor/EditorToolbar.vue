<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import UiButton from '../../../components/UiButton.vue'
import { linearSequences, useFlowClipboard } from '../../../lib/flowClip'
import { levelOf } from '../../../lib/flowEdit'
import { useFlowDebugStore } from '../../../stores/flowDebug'
import { useFlowEditorStore } from '../../../stores/flowEditor'

withDefaults(defineProps<{ saving: boolean; importing?: boolean }>(), { importing: false })
const emit = defineEmits<{ back: []; save: []; align: []; settings: []; import: []; export: []; run: []; errors: [] }>()
const { t } = useI18n()
const store = useFlowEditorStore()
const debug = useFlowDebugStore()
const clip = useFlowClipboard()

const name = computed({
  get: () => store.doc?.name ?? '',
  set: (value: string) => store.apply((doc) => (doc.name === value ? doc : { ...doc, name: value }), 'name'),
})
const errorCount = computed(() => store.report.errors.length)
const nameError = computed(() => store.report.errors.find((i) => i.node === null && i.field === 'name')?.message)
const chosen = computed(() => store.doc ? levelOf(store.doc, store.path).nodes.filter((node) => store.selection.includes(node.id)) : [])
const groupIds = computed(() => {
  if (!store.doc || store.selection.length !== 1) return store.selection
  return linearSequences(store.doc, store.path, new Map(store.catalog.ops.map((op) => [op.id, op])))
    .find((ids) => ids.includes(store.selection[0]!)) ?? store.selection
})
const canGroup = computed(() => chosen.value.some((node) => node.op !== 'start')
  && !(chosen.value.length === 1 && chosen.value[0]?.op === 'control.group'))
const canUngroup = computed(() => chosen.value.length === 1 && chosen.value[0]?.op === 'control.group')
const needsSave = computed(() => store.id === null || store.dirty)
const canRun = computed(() => !needsSave.value && errorCount.value === 0)
</script>

<template>
  <header class="toolbar">
    <UiButton variant="ghost" size="icon" icon="prev" :title="t('automation.editor.back')" @click="emit('back')"><span class="sr-only">{{ t('automation.editor.back') }}</span></UiButton>
    <input v-model="name" class="input flow-name" :class="{ invalid: nameError }" :title="nameError" :aria-invalid="nameError ? true : undefined"
      :aria-label="t('automation.editor.nameLabel')" />
    <button v-if="errorCount > 0" type="button" class="badge bad" @click="emit('errors')">{{ t('automation.editor.errorsCount', { n: errorCount }) }}</button>
    <div class="toolbar-actions" role="toolbar" :aria-label="t('automation.editor.toolbar')">
      <div class="tool-group" role="group" :aria-label="t('automation.editor.historyTools')">
        <UiButton size="icon" variant="ghost" icon="undo" :disabled="!store.canUndo" :title="t('automation.editor.undo')" @click="store.undo()"><span class="sr-only">{{ t('automation.editor.undo') }}</span></UiButton>
        <UiButton size="icon" variant="ghost" icon="redo" :disabled="!store.canRedo" :title="t('automation.editor.redo')" @click="store.redo()"><span class="sr-only">{{ t('automation.editor.redo') }}</span></UiButton>
      </div>
      <div class="tool-group" role="group" :aria-label="t('automation.editor.blockTools')">
        <UiButton size="icon" variant="ghost" icon="paste" :disabled="!clip" :title="t('automation.editor.paste', { n: clip?.nodes.length ?? 0 })"
          @click="store.paste()"><span class="sr-only">{{ t('automation.editor.paste', { n: clip?.nodes.length ?? 0 }) }}</span></UiButton>
        <UiButton size="icon" :variant="store.nodeSettingsOpen ? 'soft' : 'ghost'" icon="edit" :disabled="!chosen.length"
          :aria-expanded="store.nodeSettingsOpen" aria-controls="node-panel" :title="t(chosen.length > 1 ? 'automation.editor.selectionSettings' : 'automation.editor.nodeSettings')"
          @click="store.nodeSettingsOpen ? store.nodeSettingsOpen = false : store.openNodeSettings()"><span class="sr-only">{{ t(chosen.length > 1 ? 'automation.editor.selectionSettings' : 'automation.editor.nodeSettings') }}</span></UiButton>
        <UiButton size="icon" variant="ghost" icon="alignNodes" :title="t('automation.editor.align')" @click="emit('align')"><span class="sr-only">{{ t('automation.editor.align') }}</span></UiButton>
        <UiButton size="icon" variant="ghost" icon="groupNodes" :disabled="!canGroup"
          :title="groupIds.length > store.selection.length ? t('automation.editor.groupSequenceCount', { n: groupIds.length }) : t('automation.editor.group')"
          @click="store.group(groupIds)"><span class="sr-only">{{ t('automation.editor.group') }}</span></UiButton>
        <UiButton size="icon" variant="ghost" icon="ungroupNodes" :disabled="!canUngroup" :title="t('automation.editor.ungroup')"
          @click="store.ungroup(chosen[0]!.id)"><span class="sr-only">{{ t('automation.editor.ungroup') }}</span></UiButton>
      </div>
      <div class="tool-group" role="group" :aria-label="t('automation.editor.fileTools')">
        <UiButton size="icon" variant="ghost" icon="download" :busy="importing" :disabled="saving" :title="t('automation.editor.import')"
          @click="emit('import')"><span class="sr-only">{{ t('automation.editor.import') }}</span></UiButton>
        <UiButton size="icon" variant="ghost" icon="upload" :disabled="needsSave" :title="t(needsSave ? 'automation.editor.saveFirst' : 'automation.editor.export')"
          @click="emit('export')"><span class="sr-only">{{ t('automation.editor.export') }}</span></UiButton>
      </div>
      <div class="tool-group" role="group" :aria-label="t('automation.editor.processTools')">
        <UiButton size="icon" variant="ghost" icon="settings" :title="t('automation.editor.settings')" @click="emit('settings')"><span class="sr-only">{{ t('automation.editor.settings') }}</span></UiButton>
        <UiButton size="icon" icon="journal" :variant="debug.open ? 'soft' : 'ghost'" :aria-pressed="debug.open" :title="t('automation.editor.debugLog')"
          @click="debug.toggle()"><span class="sr-only">{{ t('automation.editor.debug') }}</span></UiButton>
        <UiButton size="icon" variant="ghost" icon="play" :disabled="!canRun" :title="t(needsSave ? 'automation.editor.saveFirst' : errorCount ? 'automation.editor.fixErrorsFirst' : 'automation.editor.run')"
          @click="emit('run')"><span class="sr-only">{{ t('automation.editor.run') }}</span></UiButton>
      </div>
      <UiButton variant="primary" size="sm" icon="save" :disabled="store.id !== null && !store.dirty" :loading="saving" @click="emit('save')">{{ t('automation.editor.save') }}</UiButton>
    </div>
  </header>
</template>

<style scoped>
.toolbar { display: flex; align-items: center; gap: 8px; flex-shrink: 0; flex-wrap: wrap; }
.flow-name { flex: 1 1 180px; min-width: 160px; max-width: 320px; width: 240px; font-weight: 500; }
.toolbar-actions { display: flex; align-items: center; gap: 8px; margin-left: auto; flex-wrap: wrap; }
.tool-group { display: flex; align-items: center; gap: 2px; padding: 3px; border: 1px solid var(--border); border-radius: 10px; background: var(--surface); }
.tool-group .btn.square { width: 30px; height: 30px; border-radius: 6px; }
.badge.bad { border: 0; background: var(--danger-bg); color: var(--danger-text); cursor: pointer; }
</style>
