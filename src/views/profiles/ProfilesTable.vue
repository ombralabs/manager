<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { GroupRecord, ProfileRow, SortKey } from '../../api/types'
import { maskedOwnerHint } from '../../api/errors'
import AppIcon from '../../components/AppIcon.vue'
import StatusPill from '../../components/StatusPill.vue'
import UiButton from '../../components/UiButton.vue'
import UiCheckbox from '../../components/UiCheckbox.vue'
import {
  type GatewayCell,
  type OptionalColumn,
  formatDateTime,
  gatewayCell,
  gridTemplate,
  isLive,
} from '../../lib/columns'
import { fingerprintTitle, kindLabel } from '../../lib/format'
import { useAppStore } from '../../stores/app'
import { useProfilesStore } from '../../stores/profiles'
import RowMenu from './RowMenu.vue'
import ProfileLabelPicker from './ProfileLabelPicker.vue'
import ProfileTagsPicker from './ProfileTagsPicker.vue'

const props = defineProps<{ rows: ProfileRow[]; hidden: OptionalColumn[]; groups: GroupRecord[]; syncMain?: string | null }>()
function usesDirect(row: ProfileRow) {
  const { status } = row
  if (status.state === 'running' || status.state === 'paused') return status.proxy_id === 0
  if (status.state === 'waiting' && status.info) return status.info.proxy_id === 0
  return row.direct === true
}
/** «Ждёт поток»: подсказку (причина) рисует сама пилюля; у остальных — «Браузер: <статус>». */
function pillAttrs(row: ProfileRow) {
  const { status } = row
  return status.state === 'waiting'
    ? { state: status.state, license: status.license, ownerHint: status.ownerHint }
    : { state: status.state, title: t('profiles.labels.browserState', { state: t(`status.${status.state}`) }) }
}
/** Причина ожидания потока — видимой подписью рядом с пилюлей; неизвестное состояние — без подписи. */
function waitReason(row: ProfileRow): string {
  const { status } = row
  if (status.state === 'waiting' && status.license === 'device_bound') {
    const account = maskedOwnerHint(status.ownerHint)
    if (account) return t('status.license.device_bound_account', { account })
  }
  return status.state === 'waiting' && te(`status.license.${status.license}`) ? t(`status.license.${status.license}`) : ''
}
const emit = defineEmits<{
  toggleRun: [row: ProfileRow]
  edit: [row: ProfileRow]
  clone: [row: ProfileRow]
  move: [row: ProfileRow, group: number | null]
  exportCookies: [row: ProfileRow]
  exportArchive: [row: ProfileRow]
  clearCache: [row: ProfileRow]
  remove: [row: ProfileRow]
}>()
const { t, te, locale } = useI18n()
const app = useAppStore()
const profiles = useProfilesStore()

const body = ref<HTMLElement | null>(null)
const header = ref<HTMLElement | null>(null)

/** Значение, которое мы сами записали в шапку: её отложенное событие `scroll` — эхо, телу его слушать нельзя (он уже мог уйти дальше). */
let echoed = -1

/** Фокус с клавиатуры прокручивает шапку (`overflow: hidden`), WKWebView шлёт `focusin` до прокрутки — слушаем `scroll`: тело догоняет шапку. */
function syncBody() {
  if (!header.value || !body.value || header.value.scrollLeft === echoed) return
  if (body.value.scrollLeft !== header.value.scrollLeft) body.value.scrollLeft = header.value.scrollLeft
}

/**
 * Шапка не прокручивается сама: её сдвиг повторяет прокрутку строк; то же значение события не порождает, цикла нет.
 * `echoed` — всегда текущее положение тела, даже если шапка уже на месте: иначе старое значение примет следующую
 * прокрутку шапки с клавиатуры (на тот же край) за эхо, и тело останется на месте.
 */
function syncHeader() {
  if (!header.value || !body.value) return
  echoed = body.value.scrollLeft
  if (header.value.scrollLeft !== echoed) header.value.scrollLeft = echoed
}

/** Новая страница или другой набор строк открывается сверху, а не на прежней позиции прокрутки. */
watch(
  () => [
    profiles.page,
    profiles.pageSize,
    profiles.only,
    profiles.filters.search,
    profiles.filters.sort,
    profiles.filters.descending,
    profiles.filters.label,
    profiles.filters.group,
    ...profiles.filters.tags,
    profiles.filters.untagged,
  ],
  () => {
    if (body.value) body.value.scrollTop = 0
  },
  { flush: 'post' },
)

const grid = computed(() => gridTemplate(props.hidden))
const show = (column: OptionalColumn) => !props.hidden.includes(column)
const HEAD: { key: string; sort?: SortKey; optional?: OptionalColumn }[] = [
  { key: 'serial', sort: 'serial', optional: 'serial' },
  { key: 'name', sort: 'name' },
  { key: 'status' },
  { key: 'gateway', optional: 'gateway' },
  { key: 'tags', optional: 'tags' },
  { key: 'fingerprint', optional: 'fingerprint' },
  { key: 'created', sort: 'created', optional: 'created' },
  { key: 'last_opened', sort: 'last_opened', optional: 'last_opened' },
]
const headColumns = computed(() => HEAD.filter((column) => !column.optional || show(column.optional)))
/** Подписи строк считаются один раз на отрисовку страницы. */
const lines = computed(() => props.rows.map((row) => ({ row, gateway: gatewayCell(row) })))
const directionIcon = computed(() => (profiles.filters.descending ? 'sortDesc' : 'sortAsc'))

function gatewayNote(cell: GatewayCell): string {
  switch (cell.note) {
    case 'unreachable':
      return t('profiles.gateway.unreachable')
    case 'replaced':
      return t('profiles.gateway.replaced')
    case 'unassigned':
      return t('profiles.gateway.willPick')
    case 'pinned':
      return cell.kind ? `${kindLabel(cell.kind)} · ${t('profiles.gateway.pinned')}` : t('profiles.gateway.pinned')
  }
}

/** Щелчок по активной колонке меняет направление, по другой — сортирует по ней по возрастанию. */
function sortBy(key: SortKey) {
  if (profiles.filters.sort === key) {
    profiles.filters.descending = !profiles.filters.descending
  } else {
    profiles.filters.sort = key
    profiles.filters.descending = false
  }
}

/** «Выбрать все» в шапке — все профили по фильтру на всех страницах. */
function toggleAll() {
  if (profiles.allSelected) profiles.clearSelection()
  else profiles.selectAll()
}
</script>

<template>
  <div class="grid-table">
    <div ref="header" class="row header" :style="{ gridTemplateColumns: grid }" @scroll="syncBody">
      <div class="check">
        <UiCheckbox
          :model-value="profiles.allSelected"
          :indeterminate="profiles.selectedCount > 0 && !profiles.allSelected"
          :disabled="profiles.visibleRows.length === 0"
          :ariaLabel="t('profiles.table.selectAll')"
          @update:model-value="toggleAll"
        />
      </div>
      <template v-for="column in headColumns" :key="column.key">
        <button
          v-if="column.sort"
          type="button"
          class="sort"
          :title="t('profiles.sortBy', { column: t(`columns.${column.key}`) })"
          @click="sortBy(column.sort)"
        >
          {{ t(`columns.${column.key}`) }}
          <AppIcon v-if="profiles.filters.sort === column.sort" :name="directionIcon" :size="14" />
        </button>
        <div v-else>{{ t(`columns.${column.key}`) }}</div>
      </template>
      <div class="actions">{{ t('columns.action') }}</div>
    </div>

    <div v-if="rows.length === 0" class="empty"><slot name="empty" /></div>

    <div v-else ref="body" class="body" @scroll="syncHeader">
      <div
        v-for="{ row, gateway } in lines"
        :key="row.id"
        :class="['row', { selected: profiles.selected.has(row.id) }]"
        :style="{ gridTemplateColumns: grid }"
      >
        <div class="check">
          <UiCheckbox
            :model-value="profiles.selected.has(row.id)"
            :ariaLabel="t('profiles.table.selectRow', { name: row.name })"
            @update:model-value="profiles.toggle(row.id)"
          />
        </div>
        <div v-if="show('serial')" class="mono muted serial">{{ row.serial }}</div>
        <div class="cell">
          <button type="button" class="main name" :aria-label="t('profiles.editProfile', { name: row.name })" :title="row.name" @click="emit('edit', row)">{{ row.name }}</button>
          <div class="sub profile-meta">
            <span v-if="row.id === syncMain" class="sync-master" :title="t('sync.masterHint')"><AppIcon name="sync" :size="14" />{{ t('sync.master') }}</span>
            <span class="profile-id">#{{ row.id.slice(0, 8) }}</span>
            <StatusPill v-bind="pillAttrs(row)" plain />
            <span v-if="waitReason(row)" class="wait-reason">{{ waitReason(row) }}</span>
          </div>
        </div>
        <div class="label-cell"><ProfileLabelPicker :row="row" /></div>
        <div v-if="show('gateway')" class="cell">
          <div class="main gateway-title" :title="gateway.title ?? undefined">
            <span v-if="gateway.note === 'unreachable' || row.gateway?.ok === false" class="proxy-warning dot-sm" />
            {{ usesDirect(row) ? t('editor.gateway.noProxy') : row.gateway ? `${row.gateway.country ?? row.gateway.host} · ${kindLabel(row.gateway.kind)}` : t('profiles.gateway.unassigned') }}
          </div>
          <div v-if="gateway.note === 'unreachable' || gateway.note === 'replaced'" :class="['sub', { alert: gateway.note === 'unreachable' }]">{{ gatewayNote(gateway) }}</div>
        </div>
        <div v-if="show('tags')" class="tags-cell"><ProfileTagsPicker :row="row" /></div>
        <div v-if="show('fingerprint')" class="cell">
          <div class="main">{{ fingerprintTitle(row.os_version, row.brand, app.engineVersion) }}</div>
          <div class="sub mono">{{ t('profiles.seed', { seed: row.seed }) }}</div>
        </div>
        <div v-if="show('created')" class="cell">
          <div class="main">{{ formatDateTime(row.created_at, locale) }}</div>
        </div>
        <div v-if="show('last_opened')" class="cell">
          <div class="main">
            {{ row.last_opened_at === null ? t('profiles.neverOpened') : formatDateTime(row.last_opened_at, locale) }}
          </div>
        </div>
        <div class="actions">
          <UiButton
            v-if="isLive(row) || row.status.state === 'waiting'"
            size="sm"
            variant="ghost"
            class="run-action"
            icon="stop"
            :icon-size="22"
            :loading="profiles.stopping.has(row.id)"
            :aria-label="t('profiles.stop')"
            :title="t('profiles.stop')"
            @click="emit('toggleRun', row)"
          >{{ t('profiles.stop') }}</UiButton>
          <UiButton
            v-else-if="row.status.state === 'starting'"
            size="icon"
            :loading="true"
            :icon-size="22"
            :aria-label="t('status.starting')"
            :title="t('status.starting')"
          />
          <UiButton
            v-else
            size="sm"
            variant="ghost"
            class="run-action start-action"
            icon="play"
            :icon-size="22"
            :loading="profiles.busy.has(row.id)"
            :aria-label="t('profiles.start')"
            :title="t('profiles.start')"
            @click="emit('toggleRun', row)"
          >{{ t('profiles.start') }}</UiButton>
          <RowMenu
            :row="row"
            :groups="groups"
            @edit="emit('edit', row)"
            @clone="emit('clone', row)"
            @move="emit('move', row, $event)"
            @export-cookies="emit('exportCookies', row)"
            @export-archive="emit('exportArchive', row)"
            @clear-cache="emit('clearCache', row)"
            @remove="emit('remove', row)"
          />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.sync-master { display: inline-flex; align-items: center; gap: 4px; color: var(--accent); font-weight: 600; }
.sort {
  display: inline-flex;
  align-items: center;
  justify-self: start;
  gap: 4px;
  padding: 0;
  border: 0;
  background: none;
  color: inherit;
  font: inherit;
  cursor: pointer;
}

.sort:hover {
  color: var(--text);
}

.sort:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
  border-radius: 4px;
}

.row.header {
  overflow: hidden;
}

.body {
  flex-grow: 1;
  min-height: 0;
  overflow: auto;
}

/*
 * Строки за пределами окна браузер не раскладывает и не рисует, пока их не прокрутят: на странице 200 строк видно
 * около 12. Высота строки для полосы прокрутки — 64 px, после первой отрисовки — настоящая. Содержимое за край строки
 * не выходит (меню и подсказки — в порталах), поэтому обрезка `contain: paint` ничего не прячет.
 * `min-width: min-content` держит строку шире окна при узкой таблице — поэтому прокрутка по горизонтали и есть,
 * и шапка повторяет её через `syncHeader`.
 */
.body .row {
  min-height: 64px;
  background: var(--surface);
  border-bottom: 1px solid var(--row-border);
  min-width: min-content;
  content-visibility: auto;
  contain-intrinsic-size: auto 64px;
}

.body .row:hover {
  background: #f8faff;
}

.body .row.selected {
  background: var(--sidebar-active);
}

.profile-id {
  color: var(--text-2);
  font-family: var(--mono);
}
.profile-meta { display: flex; align-items: center; gap: 8px; min-width: 0; }
.profile-meta :deep(.pill.plain) { gap: 4px; font-size: 11px; }
.profile-meta :deep(.pill svg) { width: 6px; height: 6px; }
.wait-reason { min-width: 0; overflow: hidden; color: var(--warn-text); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.label-cell { min-width: 0; }

.tags-cell { min-width: 0; }

.gateway-title {
  display: flex;
  align-items: center;
  gap: 7px;
}

.proxy-warning {
  background: #f59e0b;
}

.run-action {
  flex: 1;
  justify-content: flex-start;
  padding-inline: 0;
}

.start-action {
  color: var(--accent-ink);
}

.check {
  display: flex;
  align-items: center;
}

.serial {
  font-size: 13px;
}

.name {
  font-size: 14px;
  font-weight: 500;
  padding: 0;
  border: 0;
  background: transparent;
  text-align: left;
  cursor: pointer;
}

.name:hover {
  color: var(--accent-ink);
}

.name:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
  border-radius: 3px;
}

.sub.alert {
  color: var(--danger-text);
  font-weight: 500;
}

/*
 * «Действие» закреплено у правого края: при прокрутке вбок запуск и меню не уходят из виду. Ячейка забирает поле
 * строки и промежуток перед собой — прокручиваемое под ней не просвечивает. Шапка сама прокручивается, её край
 * ограничения — без поля, отсюда сдвиг на поле.
 */
.actions {
  position: sticky;
  right: 0;
  display: flex;
  align-self: stretch;
  align-items: center;
  gap: 6px;
  margin: 0 calc(-1 * var(--row-pad, 16px)) 0 -12px;
  padding: 0 var(--row-pad, 16px) 0 12px;
  background: inherit;
  box-shadow: -6px 0 8px -6px rgb(16 24 40 / 12%);
}

.header .actions {
  right: calc(-1 * var(--row-pad, 16px));
}
</style>
