<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSettingsSaveStore } from '../stores/settingsSave'
const props = defineProps<{ scope: string }>()
const { t } = useI18n()
const saves = useSettingsSaveStore()
const state = computed(() => saves.status(props.scope))
</script>

<template>
  <p class="save-status" :class="{ failed: state === 'error' }" role="status" aria-live="polite" aria-atomic="true">
    {{ t(`settings.saveStatus.${state}`) }}
  </p>
</template>

<style scoped>
.save-status { margin: 0; min-height: 20px; font-size: 13px; color: var(--text-2); }
.failed { color: var(--danger-text); }
</style>
