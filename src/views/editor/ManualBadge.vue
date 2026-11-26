<script setup lang="ts">
import { useI18n } from 'vue-i18n'

const emit = defineEmits<{ clear: [] }>()
const { t } = useI18n()
</script>

<template>
  <button type="button" class="badge" :title="t('editor.fingerprint.manualBadgeHint')" @click="emit('clear')">
    {{ t('editor.fingerprint.manualBadge') }}
  </button>
</template>
