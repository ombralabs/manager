<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { type StatusCounts, summaryParts } from '../../lib/groups'

const props = defineProps<{ counts: StatusCounts }>()
const { t } = useI18n()

/** «3 работают, 1 упал»: части с разделителями, «упал» выделен цветом статуса. */
const parts = computed(() =>
  summaryParts(props.counts).map((part, index, all) => ({
    state: part.state,
    text: t(`groups.state.${part.state}`, part.n),
    separator: index < all.length - 1 ? ', ' : '',
  })),
)
const idle = computed(() => (props.counts.total === 0 ? t('groups.state.empty') : t('groups.state.allStopped')))
</script>

<template>
  <span class="summary">
    <template v-if="parts.length === 0">{{ idle }}</template>
    <template v-else>
      <template v-for="part in parts" :key="part.state"><span :class="{ crashed: part.state === 'crashed' }">{{ part.text }}</span>{{ part.separator }}</template>
    </template>
  </span>
</template>

<style scoped>
.crashed {
  color: var(--danger-text);
  font-weight: 600;
}
</style>
