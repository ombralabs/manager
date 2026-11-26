<script setup lang="ts">
import { DropdownMenuItem, DropdownMenuSeparator } from 'reka-ui'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { GroupRecord } from '../api/types'
import type { GroupFilter } from '../stores/profiles'

const props = defineProps<{ groups: GroupRecord[]; exclude?: GroupFilter; disabled?: boolean }>()
const emit = defineEmits<{ select: [group: number | null] }>()
const { t } = useI18n()
const items = computed(() => props.groups.filter((group) => group.id !== props.exclude))
</script>

<template>
  <DropdownMenuItem v-for="group in items" :key="group.id" class="menu-item" :disabled="disabled" @select="emit('select', group.id)">
    <span class="dot dot-sm" :style="{ background: group.color }" />
    <span class="name">{{ group.name }}</span>
  </DropdownMenuItem>
  <DropdownMenuSeparator v-if="items.length > 0 && exclude !== 'ungrouped'" class="separator" />
  <DropdownMenuItem v-if="exclude !== 'ungrouped'" class="menu-item" :disabled="disabled" @select="emit('select', null)">
    {{ t('common.ungrouped') }}
  </DropdownMenuItem>
</template>

<style scoped>
.name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
