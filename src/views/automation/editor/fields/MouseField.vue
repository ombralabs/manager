<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import UiField from '../../../../components/UiField.vue'
import UiSwitch from '../../../../components/UiSwitch.vue'
import IntRangeField from './IntRangeField.vue'
import VarText from './VarText.vue'

/** `use_mouse`: выключено — поля нет; включено — `{x, y, release_ms?}` (X, Y — целое или `${…}`). */
const props = defineProps<{ value: unknown; vars: string[]; label: string }>()
const emit = defineEmits<{ update: [value: unknown] }>()
const { t } = useI18n()

const on = () => typeof props.value === 'object' && props.value !== null
const cur = () => (on() ? (props.value as Record<string, unknown>) : {})
function patch(key: string, v: unknown) {
  const next = { ...cur(), [key]: v }
  if (v === undefined) delete next[key]
  emit('update', next)
}
</script>

<template>
  <div class="mouse">
    <UiSwitch :model-value="on()" :label="label" @update:model-value="emit('update', $event ? { x: 0, y: 0 } : undefined)" />
    <template v-if="on()">
      <div class="xy">
        <UiField :label="t('automation.editor.w.mouseX')"><VarText :model-value="cur().x as number | string | undefined" :vars="vars" numeric required @update:model-value="patch('x', $event)" /></UiField>
        <UiField :label="t('automation.editor.w.mouseY')"><VarText :model-value="cur().y as number | string | undefined" :vars="vars" numeric required @update:model-value="patch('y', $event)" /></UiField>
      </div>
      <UiField :label="t('automation.editor.w.mouseRelease')">
        <IntRangeField :value="cur().release_ms" :vars="vars" optional @update="patch('release_ms', $event)" />
      </UiField>
    </template>
  </div>
</template>

<style scoped>
.mouse { display: flex; flex-direction: column; gap: 10px; }
.xy { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
</style>
