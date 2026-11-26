<script setup lang="ts">
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from 'reka-ui'
import { nextTick, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type { GroupRecord, ProfileRow } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import GroupMenuItems from '../../components/GroupMenuItems.vue'
import UiButton from '../../components/UiButton.vue'
import { isActive } from '../../lib/columns'

defineProps<{ row: ProfileRow; groups: GroupRecord[] }>()
const emit = defineEmits<{
  edit: []
  clone: []
  move: [group: number | null]
  exportCookies: []
  exportArchive: []
  clearCache: []
  remove: []
}>()
const { t } = useI18n()

/**
 * Меню reka в строке монтируется по требованию: `DropdownMenuRoot` в каждой из 200 строк — основная доля
 * времени отрисовки страницы. До первого наведения, фокуса или нажатия в строке — простая кнопка того же вида
 * и с теми же `aria`; потом — триггер reka, он остаётся до конца жизни строки.
 */
const ready = ref(false)
const open = ref(false)
const box = ref<HTMLElement | null>(null)

/** `hover` — только подготовить; `focus` — и вернуть фокус на триггер; `open` — и открыть меню. */
async function prepare(how: 'hover' | 'focus' | 'open') {
  if (ready.value) return
  ready.value = true
  if (how === 'hover') return
  await nextTick()
  box.value?.querySelector('button')?.focus()
  if (how === 'open') open.value = true
}
</script>

<template>
  <span ref="box" class="row-menu">
    <UiButton
      v-if="!ready"
      size="icon"
      variant="ghost"
      icon="more"
      aria-haspopup="menu"
      aria-expanded="false"
      :aria-label="t('common.moreActions')"
      :title="t('common.moreActions')"
      @pointerenter="prepare('hover')"
      @pointerdown.left="(e: PointerEvent) => { if (!e.ctrlKey) open = true }"
      @focus="prepare('focus')"
      @click="prepare('open')"
      @keydown.enter.prevent="prepare('open')"
      @keydown.space.prevent="prepare('open')"
      @keydown.down.prevent="prepare('open')"
    />
    <DropdownMenuRoot v-else v-model:open="open">
      <DropdownMenuTrigger as-child>
        <UiButton size="icon" variant="ghost" icon="more" :aria-label="t('common.moreActions')" :title="t('common.moreActions')" />
      </DropdownMenuTrigger>
      <DropdownMenuPortal>
        <DropdownMenuContent align="end" :side-offset="4" class="popover">
          <DropdownMenuItem class="menu-item" @select="emit('edit')">{{ t('common.edit') }}</DropdownMenuItem>
          <DropdownMenuItem class="menu-item" @select="emit('clone')">{{ t('profiles.menu.clone') }}</DropdownMenuItem>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger class="menu-item sub-trigger">
              {{ t('profiles.menu.move') }}
              <AppIcon name="next" :size="14" />
            </DropdownMenuSubTrigger>
            <DropdownMenuPortal>
              <DropdownMenuSubContent :side-offset="4" class="popover">
                <GroupMenuItems
                  :groups="groups"
                  :exclude="row.group ? row.group.id : 'ungrouped'"
                  @select="emit('move', $event)"
                />
              </DropdownMenuSubContent>
            </DropdownMenuPortal>
          </DropdownMenuSub>
          <DropdownMenuItem class="menu-item" @select="emit('exportCookies')">{{ t('bulk.export.menuItem') }}</DropdownMenuItem>
          <DropdownMenuItem class="menu-item" :disabled="isActive(row)" @select="emit('exportArchive')">
            {{ t('profiles.menu.exportArchive') }}
          </DropdownMenuItem>
          <DropdownMenuItem class="menu-item" :disabled="isActive(row)" @select="emit('clearCache')">
            {{ t('profiles.menu.clearCache') }}
          </DropdownMenuItem>
          <DropdownMenuSeparator class="separator" />
          <DropdownMenuItem class="menu-item danger" :disabled="isActive(row)" @select="emit('remove')">
            {{ t('common.delete') }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>
  </span>
</template>

<style scoped>
/* Обёртка без своего блока: кнопка остаётся прямым элементом ряда действий строки. */
.row-menu {
  display: contents;
}

.sub-trigger {
  justify-content: space-between;
}
</style>
