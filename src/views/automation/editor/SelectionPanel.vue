<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import UiButton from '../../../components/UiButton.vue'
import { levelOf, removeItems } from '../../../lib/flowEdit'
import { useFlowEditorStore } from '../../../stores/flowEditor'

const { t } = useI18n()
const store = useFlowEditorStore()
const chosen = computed(() => (store.doc ? levelOf(store.doc, store.path).nodes.filter((n) => store.selection.includes(n.id) && n.op !== 'start') : []))
/** Все выбранные отключены — кнопка включает, иначе отключает. */
const allOff = computed(() => chosen.value.length > 0 && chosen.value.every((n) => n.disabled === true))
/** Как Delete на холсте: узлы и отдельно выбранные связи одной правкой — один шаг отмены; Start остаётся. */
const remove = () => {
  store.apply((doc) => removeItems(doc, store.path, store.selection, store.edgeSelection))
  store.nodeSettingsOpen = false
}
</script>

<template>
  <div class="selection-panel">
    <h2 class="title">{{ t('automation.editor.selectedCount', { n: store.selection.length }) }}</h2>
    <div class="actions">
      <UiButton size="sm" @click="store.copy()">{{ t('automation.editor.copy') }}</UiButton>
      <UiButton size="sm" @click="store.group()">{{ t('automation.editor.group') }}</UiButton>
      <UiButton size="sm" @click="store.toggleDisabled(store.selection, !allOff)">
        {{ allOff ? t('automation.editor.enableSelected') : t('automation.editor.disableSelected') }}
      </UiButton>
      <UiButton size="sm" @click="remove">{{ t('common.delete') }}</UiButton>
    </div>
  </div>
</template>

<style scoped>
.selection-panel { display: flex; flex-direction: column; gap: 12px; padding: 16px; background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius-card); }
.title { margin: 0; font-size: 16px; font-weight: 600; }
.actions { display: flex; gap: 8px; flex-wrap: wrap; }
</style>
