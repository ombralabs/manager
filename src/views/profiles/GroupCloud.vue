<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import type { GroupRecord } from '../../api/types'
import type { GroupFilter } from '../../stores/profiles'

const props = defineProps<{ groups: GroupRecord[]; total: number; active: GroupFilter }>()
const emit = defineEmits<{ select: [group: GroupFilter]; configure: [] }>()
const { t } = useI18n()

/** Повторное нажатие на активную группу снимает фильтр. */
function pick(id: number) {
  emit('select', props.active === id ? 'all' : id)
}
</script>

<template>
  <div class="cloud" role="group" :aria-label="t('profiles.cloud.label')">
    <button
      type="button"
      :class="['chip', { on: active === 'all' }]"
      :aria-pressed="active === 'all'"
      @click="emit('select', 'all')"
    >
      {{ t('profiles.cloud.all') }}
      <span class="count mono">{{ total }}</span>
    </button>
    <button
      v-for="group in groups"
      :key="group.id"
      type="button"
      :class="['chip', { on: active === group.id }]"
      :aria-pressed="active === group.id"
      @click="pick(group.id)"
    >
      <span class="dot dot-sm" :style="{ background: group.color }" />
      <span class="name">{{ group.name }}</span>
      <span class="count mono">{{ group.profile_count }}</span>
    </button>
    <button type="button" class="link configure" @click="emit('configure')">{{ t('profiles.cloud.configure') }}</button>
  </div>
</template>

<style scoped>
.cloud {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 32px;
  padding: 0 12px;
  border: 1px solid var(--border-strong);
  border-radius: 999px;
  background: var(--surface);
  color: var(--text);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
}

/* Нажатый фильтр — тёмная пилюля, как «Все» на холсте «Профили». */
.chip.on {
  border-color: var(--text);
  background: var(--text);
  color: var(--text-inverse);
}

.chip:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}

.name {
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.count {
  font-size: 12px;
  color: var(--text-2);
}

/* Число на тёмной пилюле — светло-серое, как текст панели (холст «Профили»). */
.chip.on .count {
  color: var(--sidebar-text);
}

.configure {
  margin-left: 4px;
  font-size: 13px;
}
</style>
