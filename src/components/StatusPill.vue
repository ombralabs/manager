<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProfileStatus } from '../api/types'
import { maskedOwnerHint } from '../api/errors'

const props = withDefaults(defineProps<{ state: ProfileStatus['state']; license?: string; ownerHint?: string; compact?: boolean; plain?: boolean }>(), {
  license: undefined,
  compact: false,
  plain: false,
})
const { t, te } = useI18n()
/** Подсказка «Ждёт поток» — причина; неизвестное состояние — без подсказки. */
const tip = computed(() => {
  if (props.state !== 'waiting') return undefined
  const account = maskedOwnerHint(props.ownerHint)
  if (props.license === 'device_bound' && account) return t('status.license.device_bound_account', { account })
  return props.license && te(`status.license.${props.license}`) ? t(`status.license.${props.license}`) : undefined
})
</script>

<template>
  <span :class="['pill', state, { compact, plain }]" :title="tip">
    <svg v-if="state === 'running'" width="8" height="8" viewBox="0 0 8 8" aria-hidden="true">
      <circle cx="4" cy="4" r="4" fill="currentColor" />
    </svg>
    <svg v-else-if="state === 'stopped'" width="8" height="8" viewBox="0 0 8 8" aria-hidden="true">
      <circle cx="4" cy="4" r="3" fill="none" stroke="currentColor" stroke-width="1.6" />
    </svg>
    <svg v-else-if="state === 'starting' || state === 'waiting'" :class="{ spin: state === 'starting' }" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <path d="M5 1a4 4 0 1 1-4 4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
    </svg>
    <svg v-else-if="state === 'paused'" width="8" height="8" viewBox="0 0 8 8" aria-hidden="true">
      <rect x="1" y="1" width="2" height="6" rx="0.5" fill="currentColor" />
      <rect x="5" y="1" width="2" height="6" rx="0.5" fill="currentColor" />
    </svg>
    <svg v-else width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <path d="M5 1l4.2 8H.8z" fill="currentColor" />
    </svg>
    {{ t(`status.${state}`) }}
  </span>
</template>

<style scoped>
.pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 10px;
  border-radius: 999px;
  font-size: 12.5px;
  font-weight: 600;
  white-space: nowrap;
}

/* «Работает» — зелёная пилюля (холст «Профили», «Группы»); в выбранной строке «Профилей» — на белом (`ProfilesTable`). */
.running {
  background: var(--ok-bg);
  color: var(--ok-text);
}

/* Экран «Группы»: пилюля 26 px. */
.compact {
  height: 26px;
  padding: 0 9px;
  font-size: 12px;
}

.stopped {
  background: var(--muted-bg);
  color: var(--text-2);
}

.starting {
  background: var(--info-bg);
  color: var(--info-text);
}

.waiting,
.paused {
  background: var(--warn-bg);
  color: var(--warn-text);
}

.crashed {
  background: var(--danger-bg);
  color: var(--danger-text);
}

.plain {
  height: auto;
  padding: 0;
  gap: 10px;
  border-radius: 0;
  background: transparent;
  font-size: 13px;
  font-weight: 400;
}

.plain.running {
  color: var(--text);
}

.plain.running svg {
  color: var(--ok);
}

.plain.stopped svg {
  color: #98a2b5;
}

.plain.stopped circle {
  fill: currentColor;
  stroke: none;
}
</style>
