<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { VarDecl } from '../../api/types'
import UiField from '../../components/UiField.vue'
import UiSwitch from '../../components/UiSwitch.vue'

const model = defineModel<string | boolean>({ required: true })
const props = defineProps<{ decl: VarDecl; error?: string | null; fromTable?: boolean }>()
const { t } = useI18n()
const id = computed(() => `task-param-${props.decl.name}`)
const placeholder = computed(() => {
  const value = props.decl.default
  if (value === null || value === undefined) return ''
  return typeof value === 'string' ? value : JSON.stringify(value)
})
const json = computed(() => props.decl.type === 'array' || props.decl.type === 'object')
</script>

<template>
  <UiField :label="decl.name" :input-id="id" :hint="decl.description" :error="error">
    <template v-if="fromTable" #badge><span class="table-source">{{ t('automation.taskDialog.fromTable') }}</span></template>
    <UiSwitch v-if="decl.type === 'boolean'" v-model="model as boolean" :label="decl.name" />
    <textarea
      v-else-if="decl.type === 'text' || json"
      :id="id"
      v-model="model as string"
      class="input"
      :class="{ mono: json, invalid: error }"
      rows="3"
      spellcheck="false"
      :placeholder="placeholder"
    />
    <input
      v-else
      :id="id"
      v-model="model as string"
      class="input"
      :class="{ invalid: error }"
      type="text"
      :inputmode="decl.type === 'number' ? 'decimal' : undefined"
      autocomplete="off"
      :placeholder="placeholder"
    />
  </UiField>
</template>

<style scoped>
.table-source { color: var(--accent-ink); font-size: 12px; }
</style>
