<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { CoreErrorPayload } from '../api/types'
import { errorTitle } from '../lib/errorText'
import AppIcon from './AppIcon.vue'

const props = defineProps<{ error: CoreErrorPayload; dataDir: string }>()
const { t, locale } = useI18n()
const title = computed(() => errorTitle(props.error.code))
/** Подробность ядра написана по-русски: в английском интерфейсе её нет, подробности — в журнале (`fatal.hint`). */
const detail = computed(() => (locale.value === 'ru' && props.error.message ? props.error.message : null))
</script>

<template>
  <main class="fatal">
    <div class="card box">
      <AppIcon name="alert" :size="28" class="mark" />
      <h1 class="section-title">{{ t('fatal.title') }}</h1>
      <p>{{ title }}</p>
      <p v-if="detail" class="hint">{{ detail }}</p>
      <p class="hint mono">{{ dataDir }}</p>
      <p class="hint">{{ t('fatal.hint') }}</p>
    </div>
  </main>
</template>

<style scoped>
.fatal {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  padding: 32px;
}

.box {
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-width: 560px;
  padding: 28px 32px;
}

.box p {
  margin: 0;
}

.mark {
  color: var(--danger-text);
}
</style>
