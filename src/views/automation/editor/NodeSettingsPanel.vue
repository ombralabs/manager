<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import AppIcon from '../../../components/AppIcon.vue'
import OpIcon from '../../../components/OpIcon.vue'
import UiButton from '../../../components/UiButton.vue'
import { levelOf, opLabel } from '../../../lib/flowEdit'
import { useFlowEditorStore } from '../../../stores/flowEditor'
import NodePanel from './NodePanel.vue'

const { t, te } = useI18n()
const editor = useFlowEditorStore()
const panel = ref<HTMLElement | null>(null)
const contents = ref<HTMLElement | null>(null)
function close() { editor.nodeSettingsOpen = false }
const nodes = computed(() => editor.doc ? levelOf(editor.doc, editor.path).nodes : [])
const selected = computed(() => nodes.value.filter((node) => editor.selection.includes(node.id)))
const op = computed(() => editor.catalog.ops.find((op) => op.id === selected.value[0]?.op))
const title = computed(() => {
  if (selected.value.length > 1) return t('automation.editor.selectionSettings')
  const node = selected.value[0]
  return node ? opLabel(node.op, op.value?.adspower, te, t) : t('automation.editor.nodeSettings')
})
watch(() => selected.value.length, (count) => { if (!count) close() })
watch(() => selected.value.map((node) => node.id).join(','), () => {
  void nextTick(() => contents.value?.scrollTo(0, 0))
})
watch(() => editor.nodeSettingsOpen, (open) => {
  if (open) {
    void nextTick(() => panel.value?.focus({ preventScroll: true }))
  } else {
    // Числовые поля отдают черновик на blur — до удаления панели из DOM.
    const active = document.activeElement
    if (active instanceof HTMLElement && panel.value?.contains(active)) {
      active.blur()
      void nextTick(() => document.querySelector<HTMLElement>('.flow-editor .canvas')?.focus({ preventScroll: true }))
    }
  }
}, { flush: 'sync' })
</script>

<template>
  <aside v-if="editor.nodeSettingsOpen" id="node-panel" ref="panel" class="node-settings" tabindex="-1"
    aria-labelledby="node-settings-title" @keydown.esc.stop="close">
    <header class="head">
      <span class="op-icon-badge" aria-hidden="true">
        <OpIcon v-if="selected.length === 1" :op="selected[0]!.op" :size="18" />
        <AppIcon v-else name="groupNodes" :size="18" />
      </span>
      <h2 id="node-settings-title" class="title">{{ title }}</h2>
      <UiButton size="icon" variant="ghost" icon="close" :icon-size="20" :aria-label="t('common.close')"
        :title="t('common.close')" @click="close" />
    </header>
    <div ref="contents" class="contents"><NodePanel embedded /></div>
  </aside>
</template>

<style scoped>
.node-settings { position: absolute; top: 12px; right: 12px; bottom: 12px; z-index: 10; display: flex; flex-direction: column; width: 360px; max-width: calc(100% - 24px); min-height: 0; border-radius: 10px; background: var(--surface); box-shadow: 0 4px 24px color-mix(in srgb, var(--text) 10%, transparent); overflow: hidden; }
.head { display: flex; flex-shrink: 0; align-items: center; gap: 8px; padding: 12px 12px 12px 16px; }
.op-icon-badge { display: grid; place-items: center; flex-shrink: 0; width: 24px; height: 24px; border-radius: 6px; color: var(--text-inverse); background: linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 35%, white)); }
.title { flex: 1; min-width: 0; margin: 0; font-size: 18px; line-height: 24px; font-weight: 600; overflow-wrap: anywhere; }
.contents { --control-h: 40px; min-height: 0; padding: 16px; overflow-y: auto; overscroll-behavior: contain; }
#node-panel :deep(.selection-panel) { padding: 0; border: 0; background: transparent; }
</style>
