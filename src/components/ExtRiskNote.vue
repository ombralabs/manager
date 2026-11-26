<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ExtensionRecord } from '../api/types'
import AppIcon from './AppIcon.vue'

/** Расширения с правом `proxy` или `privacy`: их настройки главнее флагов движка (спецификация 3Б). Пусто — ничего. */
const props = defineProps<{ extensions: ExtensionRecord[] }>()
const { t } = useI18n()
const text = computed(() =>
  t(props.extensions.length === 1 ? 'extensions.riskOne' : 'extensions.riskMany', {
    names: props.extensions.map((ext) => ext.name).join(', '),
  }),
)
</script>

<template>
  <p v-if="extensions.length > 0" class="notice risk-note"><AppIcon name="alert" :size="16" />{{ text }}</p>
</template>

<style scoped>
.risk-note {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin: 0;
}

.risk-note .icon {
  flex-shrink: 0;
  margin-top: 1px;
}
</style>
