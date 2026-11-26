<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuRoot, DropdownMenuSeparator, DropdownMenuTrigger } from 'reka-ui'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ExtensionRecord } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import UiButton from '../../components/UiButton.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { sourceLabel } from '../../lib/extensions'
import type { PendingExtension } from '../../stores/extensions'
import ExtTile from './ExtTile.vue'

/**
 * Карточка вкладки «Все расширения» (холст «Расширения»). `ext` — запись библиотеки; без неё — карточка «Загружается»
 * для `pending`. `sets` — наборы с расширением, кроме набора 0. `saving` — состав набора 0 сохраняется: переключатель
 * `aria-disabled` (щелчок игнорирует экран). `updating` — расширение, которое сейчас обновляется из Web Store.
 */
const props = withDefaults(
  defineProps<{
    ext?: ExtensionRecord
    pending?: PendingExtension
    enabled?: boolean
    sets?: string[]
    saving?: boolean
    updating?: string | null
  }>(),
  { ext: undefined, pending: undefined, enabled: false, sets: () => [], saving: false, updating: null },
)
const emit = defineEmits<{ toggle: [on: boolean]; update: []; remove: []; openStore: [] }>()
const { t } = useI18n()

const name = computed(() => props.ext?.name ?? props.pending?.name ?? '')
const tileId = computed(() => props.ext?.id ?? props.pending?.key ?? '')
/** У загружающегося источник по способу добавления, имени файла ещё нет. */
/** Строка «Источник…» обрезается — полный текст в подсказке. */
const sourceTitle = computed(() => `${t('extensions.card.source')} ${source.value}${props.ext ? ` · ${setsText.value}` : ''}`)
const source = computed(() => sourceLabel(props.ext ?? { source: props.pending?.from ?? 'file', file_name: null }))
const setsText = computed(() =>
  props.sets.length > 0 ? t('extensions.card.sets', { sets: props.sets.join(', ') }) : t('extensions.card.noSets'),
)
</script>

<template>
  <article class="ext-card card" data-test="ext-card">
    <div class="top">
      <ExtTile :id="tileId" :name="name" />
      <div class="heading">
        <h2 class="name">
          <span class="name-text" :title="name">{{ name }}</span>
          <span v-if="ext?.gateway_risk" class="risk" :title="t('extensions.risk')">
            <AppIcon name="alert" :size="16" /><span class="sr-only">{{ t('extensions.risk') }}</span>
          </span>
        </h2>
        <div class="sub mono">
          {{ ext ? t('extensions.card.version', { version: ext.version }) : t(pending?.from === 'store' ? 'extensions.card.fromStore' : 'extensions.card.fromFile') }}
        </div>
      </div>
      <div class="state">
        <UiSwitch
          v-if="ext"
          :model-value="enabled"
          :label="t(enabled ? 'extensions.card.on' : 'extensions.card.off', { name })"
          :aria-disabled="saving || undefined"
          @update:model-value="emit('toggle', $event)"
        />
        <UiSwitch v-else :model-value="false" :label="t('extensions.card.loading', { name })" disabled />
        <span v-if="ext" class="status ok"><AppIcon name="check" :size="14" />{{ t('extensions.card.available') }}</span>
        <span v-else class="status loading"><AppIcon name="spinner" :size="12" class="spin" />{{ t('extensions.card.loadingStatus') }}</span>
      </div>
    </div>
    <p class="description" :title="ext?.description || undefined">{{ ext?.description }}</p>
    <div class="foot">
      <div class="source" :title="sourceTitle">
        {{ t('extensions.card.source') }}
        <button v-if="ext?.source === 'store'" type="button" class="link" @click="emit('openStore')">{{ source }}</button>
        <template v-else>{{ source }}</template>
        <template v-if="ext"> · {{ setsText }}</template>
      </div>
      <DropdownMenuRoot v-if="ext">
        <DropdownMenuTrigger as-child>
          <UiButton
            size="icon"
            variant="ghost"
            icon="more"
            :loading="updating === ext.id"
            :aria-label="t('extensions.card.more', { name })"
            :title="t('common.moreActions')"
          />
        </DropdownMenuTrigger>
        <DropdownMenuPortal>
          <DropdownMenuContent align="end" :side-offset="4" class="popover">
            <template v-if="ext.source === 'store'">
              <DropdownMenuItem class="menu-item" :disabled="updating !== null" @select="emit('update')">{{ t('extensions.update') }}</DropdownMenuItem>
              <DropdownMenuSeparator class="separator" />
            </template>
            <DropdownMenuItem class="menu-item danger" :disabled="updating === ext.id" @select="emit('remove')">{{ t('extensions.remove') }}</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenuPortal>
      </DropdownMenuRoot>
    </div>
  </article>
</template>

<style scoped>
/* Размеры и отступы — артборд «Расширения». */
.ext-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  padding: 18px 20px;
}

.top {
  display: flex;
  align-items: flex-start;
  gap: 14px;
}

.heading {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex-grow: 1;
  min-width: 0;
  padding-top: 2px;
}

.name {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  margin: 0;
  font-size: 15px;
  line-height: 22px;
  font-weight: 600;
}

.name-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.risk {
  display: inline-flex;
  flex-shrink: 0;
  color: var(--warn-text);
}

.sub {
  font-size: 12px;
  line-height: 16px;
  color: var(--text-2);
  white-space: nowrap;
}

.state {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 6px;
  flex-shrink: 0;
}

.status {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 12px;
  line-height: 18px;
  font-weight: 600;
  white-space: nowrap;
}

.status.ok {
  color: var(--ok-text);
}

.status.loading {
  color: var(--info-text);
}

/* Две строки описания; длиннее — многоточие, целиком — в `title`. */
.description {
  display: -webkit-box;
  height: 36px;
  margin: 0;
  overflow: hidden;
  font-size: 13px;
  line-height: 18px;
  color: var(--text-2);
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  height: 36px;
}

.source {
  min-width: 0;
  overflow: hidden;
  font-size: 13px;
  line-height: 18px;
  color: var(--text-2);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.source .link {
  font-weight: 500;
}
</style>
