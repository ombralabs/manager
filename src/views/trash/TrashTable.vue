<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuRoot, DropdownMenuSeparator, DropdownMenuTrigger } from 'reka-ui'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProfileRow } from '../../api/types'
import GroupChip from '../../components/GroupChip.vue'
import UiButton from '../../components/UiButton.vue'
import UiCheckbox from '../../components/UiCheckbox.vue'
import { formatDateTime } from '../../lib/columns'

const props = defineProps<{ rows: ProfileRow[]; selected: ReadonlySet<string>; busy: boolean }>()
const emit = defineEmits<{
  toggle: [id: string]
  togglePage: [value: boolean]
  restore: [row: ProfileRow]
  purge: [row: ProfileRow]
}>()
const { t, locale } = useI18n()

const pageSelected = computed(() => props.rows.length > 0 && props.rows.every((row) => props.selected.has(row.id)))
const pagePartly = computed(() => !pageSelected.value && props.rows.some((row) => props.selected.has(row.id)))
</script>

<template>
  <div class="grid-table">
    <div class="row header">
      <div class="check">
        <UiCheckbox
          :model-value="pageSelected"
          :indeterminate="pagePartly"
          :disabled="rows.length === 0"
          :ariaLabel="t('trash.selectPage')"
          @update:model-value="emit('togglePage', $event)"
        />
      </div>
      <div>{{ t('columns.serial') }}</div>
      <div>{{ t('columns.name') }}</div>
      <div>{{ t('columns.group') }}</div>
      <div>{{ t('trash.deletedColumn') }}</div>
      <div><span class="sr-only">{{ t('columns.action') }}</span></div>
    </div>

    <div v-if="rows.length === 0" class="empty"><slot name="empty" /></div>

    <div v-else class="body">
      <div v-for="row in rows" :key="row.id" :class="['row', { selected: selected.has(row.id) }]">
        <div class="check">
          <UiCheckbox
            :model-value="selected.has(row.id)"
            :ariaLabel="t('trash.selectRow', { name: row.name })"
            @update:model-value="emit('toggle', row.id)"
          />
        </div>
        <div class="mono muted">{{ row.serial }}</div>
        <div class="cell"><div class="main">{{ row.name }}</div></div>
        <div class="cell"><GroupChip :group="row.group" /></div>
        <div class="cell"><div class="main">{{ row.deleted_at === null ? '' : formatDateTime(row.deleted_at, locale) }}</div></div>
        <div class="actions">
          <DropdownMenuRoot>
            <DropdownMenuTrigger as-child>
              <UiButton size="icon" variant="ghost" icon="more" :aria-label="t('common.moreActions')" :title="t('common.moreActions')" />
            </DropdownMenuTrigger>
            <DropdownMenuPortal>
              <DropdownMenuContent align="end" :side-offset="4" class="popover">
                <DropdownMenuItem class="menu-item" :disabled="busy" @select="emit('restore', row)">{{ t('trash.restore') }}</DropdownMenuItem>
                <DropdownMenuSeparator class="separator" />
                <DropdownMenuItem class="menu-item danger" :disabled="busy" @select="emit('purge', row)">{{ t('trash.purge') }}</DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenuPortal>
          </DropdownMenuRoot>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.row {
  grid-template-columns: 44px 64px minmax(0, 1fr) 200px 160px 44px;
}

.body {
  flex-grow: 1;
  min-height: 0;
  overflow: auto;
}

.body .row {
  min-height: 52px;
  border-bottom: 1px solid var(--row-border);
}

.body .row:last-child {
  border-bottom: 0;
}

.check,
.actions {
  display: flex;
  align-items: center;
}

.actions {
  justify-content: flex-end;
}
</style>
