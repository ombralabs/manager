<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ExtCategory, ExtensionRecord } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { sourceLabel } from '../../lib/extensions'
import type { PendingExtension } from '../../stores/extensions'
import ExtTile from './ExtTile.vue'

/**
 * Состав набора (холст «Расширения: наборы»): вся библиотека, переключатель — «в наборе». `enabled` — состав набора 0:
 * у остальных пометка «только в наборах». `saving` — состав сохраняется: переключатели `aria-disabled`.
 */
const props = defineProps<{
  set: ExtCategory
  library: ExtensionRecord[]
  enabled: ReadonlySet<string>
  pending: PendingExtension[]
  saving: boolean
}>()
const emit = defineEmits<{ toggle: [id: string, on: boolean] }>()
const { t } = useI18n()

const members = computed(() => new Set(props.set.extension_ids))
const inSet = computed(() => props.library.filter((ext) => members.value.has(ext.id)).length)
</script>

<template>
  <section class="composition card" :aria-label="t('extensions.composition.label')">
    <div class="card-head">
      <h2 class="section-title title">{{ t('extensions.composition.title', { name: set.name }) }}</h2>
      <div class="hint">{{ t('extensions.composition.count', { n: inSet, m: library.length }) }}</div>
    </div>
    <ul class="rows">
      <li v-for="ext in library" :key="ext.id" class="row" data-test="composition-row">
        <ExtTile :id="ext.id" :name="ext.name" :size="36" />
        <div class="text">
          <div class="name">
            <span class="name-text" :title="ext.name">{{ ext.name }}</span>
            <span v-if="ext.gateway_risk" class="risk" :title="t('extensions.risk')">
              <AppIcon name="alert" :size="14" /><span class="sr-only">{{ t('extensions.risk') }}</span>
            </span>
          </div>
          <div class="line">{{ t('extensions.composition.line', { source: sourceLabel(ext), version: ext.version }) }}</div>
        </div>
        <span v-if="!enabled.has(ext.id)" class="badge" :title="t('extensions.composition.onlyInSetsTitle')">
          {{ t('extensions.composition.onlyInSets') }}
        </span>
        <UiSwitch
          :model-value="members.has(ext.id)"
          :label="t(members.has(ext.id) ? 'extensions.composition.in' : 'extensions.composition.out', { name: ext.name })"
          :aria-disabled="saving || undefined"
          @update:model-value="emit('toggle', ext.id, $event)"
        />
      </li>
      <li v-for="item in pending" :key="`pending-${item.key}`" class="row" data-test="composition-row">
        <ExtTile :id="item.key" :name="item.name" :size="36" />
        <div class="text">
          <div class="name"><span class="name-text" :title="item.name">{{ item.name }}</span></div>
          <div class="line">{{ sourceLabel({ source: item.from, file_name: null }) }}</div>
        </div>
        <span class="status"><AppIcon name="spinner" :size="12" class="spin" />{{ t('extensions.card.loadingStatus') }}</span>
        <UiSwitch :model-value="false" :label="t('extensions.composition.loading', { name: item.name })" disabled />
      </li>
    </ul>
    <p class="foot hint">{{ t('extensions.composition.hint') }}</p>
  </section>
</template>

<style scoped>
/* Размеры — артборд «Расширения: наборы». */
.composition {
  display: flex;
  flex-direction: column;
  flex-grow: 1;
  align-self: flex-start;
  min-width: 0;
  max-height: 100%;
  overflow: hidden;
}

.card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  flex-shrink: 0;
  height: 56px;
  padding: 0 16px;
  border-bottom: 1px solid var(--border);
  white-space: nowrap;
}

.title {
  overflow: hidden;
  text-overflow: ellipsis;
}

.rows {
  margin: 0;
  padding: 0;
  overflow: auto;
  list-style: none;
}

.row {
  display: flex;
  align-items: center;
  gap: 12px;
  height: 60px;
  padding: 0 16px;
  border-bottom: 1px solid var(--row-border);
}

.row:last-child {
  border-bottom: 0;
}

.text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex-grow: 1;
  min-width: 0;
}

.name {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-weight: 500;
}

.name-text,
.line {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.line {
  font-size: 12px;
  line-height: 16px;
  color: var(--text-2);
}

.risk {
  display: inline-flex;
  flex-shrink: 0;
  color: var(--warn-text);
}

.badge {
  flex-shrink: 0;
  padding: 0 8px;
  border-radius: 999px;
  background: var(--muted-bg);
  color: var(--text-2);
  font-size: 12px;
  line-height: 22px;
  font-weight: 600;
  white-space: nowrap;
}

.status {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  flex-shrink: 0;
  color: var(--info-text);
  font-size: 12px;
  line-height: 18px;
  font-weight: 600;
  white-space: nowrap;
}

.foot {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  min-height: 44px;
  margin: 0;
  padding: 0 16px;
  border-top: 1px solid var(--border);
}
</style>
