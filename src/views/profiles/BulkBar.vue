<script setup lang="ts">
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuTrigger,
} from 'reka-ui'
import { useI18n } from 'vue-i18n'
import type { ExtCategory, GroupRecord } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import GroupMenuItems from '../../components/GroupMenuItems.vue'
import UiButton from '../../components/UiButton.vue'
import { setLabel } from '../../lib/extensions'

defineProps<{ count: number; groups: GroupRecord[]; sets: ExtCategory[]; riskySets: ReadonlySet<number>; busy: boolean }>()
const emit = defineEmits<{
  clear: []
  start: []
  stop: []
  sync: []
  runFlow: []
  move: [group: number | null]
  extSet: [category: number]
  clone: []
  export: []
  edit: []
  remove: []
}>()
const { t } = useI18n()
</script>

<template>
  <div :class="['bulk', { 'has-selection': count > 0 }]" role="toolbar" :aria-label="t('profiles.bulk.label')">
    <span class="selected">{{ t('profiles.bulk.selected', { n: count }) }}</span>
    <div class="buttons">
      <UiButton size="sm" :disabled="!count" icon="play" class="keep" :title="t('profiles.bulk.start')" :busy="busy" @click="emit('start')">
        <span class="label">{{ t('profiles.bulk.start') }}</span>
      </UiButton>
      <UiButton size="sm" :disabled="!count" icon="stop" class="keep" :title="t('profiles.bulk.stop')" :busy="busy" @click="emit('stop')">
        <span class="label">{{ t('profiles.bulk.stop') }}</span>
      </UiButton>
      <UiButton size="sm" :disabled="count < 2" icon="sync" :title="t('profiles.bulk.sync')" :busy="busy" @click="emit('sync')">
        <span class="label">{{ t('profiles.bulk.sync') }}</span>
      </UiButton>
      <UiButton size="sm" :disabled="!count" icon="automation" :title="t('profiles.bulk.runFlow')" :busy="busy" @click="emit('runFlow')">
        <span class="label">{{ t('profiles.bulk.runFlow') }}</span>
      </UiButton>
      <DropdownMenuRoot>
        <DropdownMenuTrigger as-child>
          <UiButton size="sm" :disabled="!count" icon="moveToFolder" :title="t('profiles.bulk.move')">
            <span class="label">{{ t('profiles.bulk.move') }}</span>
          </UiButton>
        </DropdownMenuTrigger>
        <DropdownMenuPortal>
          <DropdownMenuContent align="start" :side-offset="4" class="popover">
            <GroupMenuItems :groups="groups" :disabled="busy" @select="emit('move', $event)" />
          </DropdownMenuContent>
        </DropdownMenuPortal>
      </DropdownMenuRoot>
      <DropdownMenuRoot>
        <DropdownMenuTrigger as-child>
          <UiButton size="sm" :disabled="!count" icon="extensions" :title="t('profiles.bulk.extensions')"><span class="label">{{ t('profiles.bulk.extensions') }}</span></UiButton>
        </DropdownMenuTrigger>
        <DropdownMenuPortal>
          <DropdownMenuContent align="start" :side-offset="4" class="popover">
            <DropdownMenuLabel class="menu-note hint">{{ t('profiles.bulk.extensionsMenu') }}</DropdownMenuLabel>
            <DropdownMenuItem v-for="set in sets" :key="set.id" class="menu-item" :disabled="busy" @select="emit('extSet', set.id)">
              <span class="name">{{ setLabel(set) }}</span>
              <span v-if="riskySets.has(set.id)" class="risk" :title="t('extensions.setRisky')">
                <AppIcon name="alert" :size="16" /><span class="sr-only">{{ t('extensions.setRisky') }}</span>
              </span>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenuPortal>
      </DropdownMenuRoot>
      <UiButton size="sm" :disabled="!count" icon="copy" :title="t('profiles.bulk.clone')" :busy="busy" @click="emit('clone')"><span class="label">{{ t('profiles.bulk.clone') }}</span></UiButton>
      <UiButton size="sm" :disabled="!count" icon="cookieExport" :title="t('bulk.export.button')" :busy="busy" @click="emit('export')"><span class="label">{{ t('bulk.export.button') }}</span></UiButton>
      <UiButton size="sm" :disabled="!count" icon="edit" :title="t('bulk.edit.button')" :busy="busy" @click="emit('edit')"><span class="label">{{ t('bulk.edit.button') }}</span></UiButton>
    </div>
    <div class="spacer" />
    <UiButton size="sm" :disabled="!count" variant="ghost" icon="deselect" :title="t('profiles.bulk.clear')" @click="emit('clear')"><span class="label">{{ t('profiles.bulk.clear') }}</span></UiButton>
    <UiButton size="sm" :disabled="!count" variant="danger" icon="trash" :title="t('profiles.bulk.delete')" :busy="busy" @click="emit('remove')"><span class="label">{{ t('profiles.bulk.delete') }}</span></UiButton>
  </div>
</template>

<style scoped>
/*
 * Панель не переносится на новые строки: узкая прячет подписи, остаются значки с подсказкой (подпись остаётся для
 * чтения с экрана). Сначала — все, кроме «Запустить» и «Остановить», потом и их. Пороги — ширина панели с русскими
 * подписями (самые длинные) плюс запас на число выбранных; перенос строк — на крайний случай.
 */
.bulk {
  container: bulk / inline-size;
  display: flex;
  flex-shrink: 0;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
  min-height: 48px;
  padding: 6px 16px;
  border-bottom: 1px solid var(--border);
  background: var(--surface);
}

.selected {
  color: var(--text-2);
  font-weight: 500;
}

.bulk.has-selection {
  background: var(--accent-soft);
}

.has-selection .selected {
  color: var(--text);
}

.buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.has-selection :deep(.secondary) {
  border-color: var(--bulk-border);
}

.spacer {
  flex-grow: 1;
}

@container bulk (width < 1720px) {
  .bulk .btn:not(.keep) {
    width: 36px;
    padding: 0;
  }

  .bulk .btn:not(.keep) .label {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
}

@container bulk (width < 760px) {
  .bulk .btn {
    width: 36px;
    padding: 0;
  }

  .bulk .btn .label {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
}

.risk {
  display: inline-flex;
  margin-left: auto;
  color: var(--warn-text);
}
</style>
