<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProxyRow } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { kindLabel } from '../../lib/format'
import { CHECK_ICON, checkState, countryIp, pinnedLabel, proxyAddress } from '../../lib/proxies'
import ProxyRowMenu from './ProxyRowMenu.vue'

const props = defineProps<{ rows: ProxyRow[]; checking: ReadonlySet<number> }>()
const emit = defineEmits<{
  toggle: [row: ProxyRow, enabled: boolean]
  check: [row: ProxyRow]
  edit: [row: ProxyRow]
  remove: [row: ProxyRow]
}>()
const { t } = useI18n()

/** Подписи строк считаются один раз на отрисовку списка. */
const lines = computed(() =>
  props.rows.map((row) => ({
    row,
    address: proxyAddress(row),
    geo: countryIp(row),
    state: checkState(row),
    pinned: pinnedLabel(row),
  })),
)
</script>

<template>
  <div class="grid-table">
    <div class="row header">
      <div>{{ t('proxies.columns.kind') }}</div>
      <div>{{ t('proxies.columns.address') }}</div>
      <div>{{ t('proxies.columns.countryIp') }}</div>
      <div>{{ t('proxies.columns.check') }}</div>
      <div>{{ t('proxies.columns.pinned') }}</div>
      <div class="right">{{ t('proxies.columns.enabled') }}</div>
      <div><span class="sr-only">{{ t('proxies.columns.actions') }}</span></div>
    </div>

    <div v-if="rows.length === 0" class="empty"><slot name="empty" /></div>

    <div v-else class="body">
      <div v-for="line in lines" :key="line.row.id" :class="['row', { off: !line.row.enabled }]">
        <div class="kind">{{ kindLabel(line.row.kind) }}</div>
        <div class="cell">
          <div class="main mono">{{ line.address }}</div>
          <div v-if="line.row.note" class="sub">{{ line.row.note }}</div>
        </div>
        <div class="cell">
          <div :class="['main', line.geo === null ? 'muted' : 'mono']">{{ line.geo ?? t('proxies.unknown') }}</div>
        </div>
        <div class="state">
          <template v-if="checking.has(line.row.id)">
            <AppIcon name="spinner" :size="16" class="spin" />
            <span class="muted">{{ t('proxies.state.checking') }}</span>
          </template>
          <span v-else :class="['state-label', line.state]">
            <AppIcon :name="CHECK_ICON[line.state]" :size="16" />
            {{ t(`proxies.state.${line.state}`) }}
          </span>
        </div>
        <div class="cell">
          <div :class="['main', { muted: line.row.profiles === 0 }]">{{ line.pinned }}</div>
          <div v-if="line.row.used_by_name" class="sub">{{ t('proxies.runningBy', { name: line.row.used_by_name }) }}</div>
        </div>
        <div class="end">
          <UiSwitch
            :model-value="line.row.enabled"
            :label="t('proxies.enabledFor', { address: line.address })"
            @update:model-value="emit('toggle', line.row, $event)"
          />
        </div>
        <div class="actions">
          <ProxyRowMenu
            :checking="checking.has(line.row.id)"
            @check="emit('check', line.row)"
            @edit="emit('edit', line.row)"
            @remove="emit('remove', line.row)"
          />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Сетка холста `64px 1fr 150px 124px 150px 44px` и колонка меню строки. */
.row {
  grid-template-columns: 64px minmax(0, 1fr) 150px 124px 150px 44px 44px;
}

.body {
  flex-grow: 1;
  min-height: 0;
  overflow: auto;
}

.body .row {
  min-height: 56px;
  border-bottom: 1px solid var(--row-border);
}

.body .row:last-child {
  border-bottom: 0;
}

.kind {
  font-size: 12px;
  font-weight: 600;
}

.row.off .kind,
.row.off .main {
  color: var(--text-2);
}

.state,
.state-label {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-size: 13px;
  white-space: nowrap;
}

.state-label.ok {
  color: var(--ok-text);
  font-weight: 500;
}

.state-label.failed {
  color: var(--danger-text);
  font-weight: 600;
}

.state-label.unchecked {
  color: var(--text-2);
}

.end {
  display: flex;
  justify-content: flex-end;
}

.actions {
  display: flex;
  justify-content: flex-end;
}
</style>
