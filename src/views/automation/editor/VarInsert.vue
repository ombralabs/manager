<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

defineProps<{ vars: string[] }>()
const emit = defineEmits<{ pick: [name: string] }>()
const { t } = useI18n()
const open = ref(false)
function pick(name: string) {
  open.value = false
  emit('pick', name)
}
</script>

<template>
  <span class="var-insert-wrap">
    <button type="button" class="var-insert" :title="t('automation.editor.insertVar')" :aria-label="t('automation.editor.insertVar')"
      @mousedown.prevent @click="open = !open">{x}</button>
    <div v-if="open" class="var-menu popover">
      <button v-for="name in vars" :key="name" type="button" class="menu-item mono" @click="pick(name)">{{ name }}</button>
      <div v-if="vars.length === 0" class="hint">{{ t('automation.editor.noVars') }}</div>
    </div>
  </span>
</template>

<style scoped>
.var-insert-wrap { position: relative; }
.var-insert { font: 12px var(--font-mono, monospace); color: var(--text-2); background: none; border: 0; padding: 0 2px; cursor: pointer; }
.var-insert:hover { color: var(--accent); }
.var-menu { position: absolute; right: 0; top: 22px; z-index: 20; max-height: 220px; min-width: 160px; overflow: auto; padding: 4px; display: flex; flex-direction: column; }
.menu-item { text-align: left; background: none; border: 0; cursor: pointer; }
.hint { padding: 4px 8px; }
</style>
