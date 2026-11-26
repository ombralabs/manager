<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { ConnRecord, TrafficSummary } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import { notify } from '../../lib/notify'
import { createRefresher } from '../../lib/refresher'
import { formatBytes, formatDuration, formatWhen, resultText } from '../../lib/traffic'

const props = defineProps<{ profileId: string; live: boolean }>()
const { t } = useI18n()

/** Опрос работающего профиля. */
const POLL_MS = 5000
const SHORT = 100
const LONG = 500

const summary = ref<TrafficSummary | null>(null)
const records = ref<ConnRecord[]>([])
const limit = ref(SHORT)

// ponytail: ядро при каждом запросе читает журнал целиком (до 20 МБ с ротацией); кэш сумм — когда понадобится.
const refresher = createRefresher(
  async () => Promise.all([core.trafficSummary(props.profileId), core.connectionLog(props.profileId, limit.value)]),
  ([nextSummary, nextRecords]) => {
    summary.value = nextSummary
    // Ядро отдаёт порядок файла — старые первыми; на экране новые сверху.
    records.value = [...nextRecords].reverse()
  },
)

/** Первый запрос и «Обновить» — с уведомлением об ошибке; фоновые — молча, следующий повторит. */
function refreshLoud() {
  refresher.refresh().catch((e) => notify.error(e))
}

let timer: ReturnType<typeof setInterval> | undefined
function stopPolling() {
  clearInterval(timer)
  timer = undefined
}
function startPolling() {
  stopPolling()
  timer = setInterval(() => refresher.refresh().catch(() => undefined), POLL_MS)
}

watch(
  () => props.live,
  (live) => {
    if (live) startPolling()
    else stopPolling()
    // Смена состояния — сразу свежие данные: итоги остановленного запуска или счётчики нового.
    refresher.refresh().catch(() => undefined)
  },
)
onMounted(() => {
  refreshLoud()
  if (props.live) startPolling()
})
onBeforeUnmount(() => {
  stopPolling()
  refresher.cancel()
})

function showMore() {
  limit.value = LONG
  refreshLoud()
}

const since = computed(() =>
  summary.value?.since == null ? t('editor.traffic.noData') : formatWhen(summary.value.since),
)
</script>

<template>
  <div class="stack">
    <section class="card section">
      <div class="tiles">
        <div class="tile">
          <div class="hint">{{ t('editor.traffic.sent') }}</div>
          <div class="value mono">{{ summary ? formatBytes(summary.bytes_up) : '—' }}</div>
        </div>
        <div class="tile">
          <div class="hint">{{ t('editor.traffic.received') }}</div>
          <div class="value mono">{{ summary ? formatBytes(summary.bytes_down) : '—' }}</div>
        </div>
        <div class="tile">
          <div class="hint">{{ t('editor.traffic.connections') }}</div>
          <div class="value mono">{{ summary?.connections ?? '—' }}</div>
        </div>
        <div class="tile">
          <div class="hint">{{ t('editor.traffic.since') }}</div>
          <div class="value">{{ since }}</div>
        </div>
      </div>
      <div class="actions">
        <span v-if="summary?.live" class="live">{{ t('editor.traffic.live') }}</span>
        <UiButton v-if="!live" size="sm" icon="refresh" :loading="refresher.loading.value" @click="refreshLoud">
          {{ t('editor.traffic.refresh') }}
        </UiButton>
      </div>
    </section>

    <section class="card section">
      <h2 class="section-title">{{ t('editor.traffic.logTitle') }}</h2>
      <div v-if="records.length === 0" class="hint">{{ t('editor.traffic.empty') }}</div>
      <div v-else class="grid-table">
        <div class="row header">
          <div>{{ t('editor.traffic.columns.time') }}</div>
          <div>{{ t('editor.traffic.columns.target') }}</div>
          <div class="right">{{ t('editor.traffic.columns.sent') }}</div>
          <div class="right">{{ t('editor.traffic.columns.received') }}</div>
          <div class="right">{{ t('editor.traffic.columns.duration') }}</div>
          <div>{{ t('editor.traffic.columns.result') }}</div>
        </div>
        <div class="body">
          <div v-for="(record, index) in records" :key="`${record.ts_ms}-${index}`" class="row">
            <div class="mono">{{ formatWhen(record.ts_ms) }}</div>
            <div class="mono target" :title="record.target">{{ record.target || '—' }}</div>
            <div class="mono right">{{ formatBytes(record.bytes_up) }}</div>
            <div class="mono right">{{ formatBytes(record.bytes_down) }}</div>
            <div class="mono right">{{ formatDuration(record.ms) }}</div>
            <div :class="['result', { failed: record.result !== 'ok' }]" :title="record.result">{{ resultText(record.result) }}</div>
          </div>
        </div>
      </div>
      <UiButton v-if="records.length >= SHORT && limit < LONG" class="more" size="sm" @click="showMore">{{ t('editor.traffic.showMore') }}</UiButton>
    </section>
  </div>
</template>

<style scoped>
.stack {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.tiles {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 16px;
}

.value {
  font-size: 18px;
  font-weight: 600;
}

.actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
}

.live {
  color: var(--ok-text);
  font-weight: 600;
}

/* Адрес — главное в журнале: ширину берём у отступов и цифровых колонок. */
.grid-table .row {
  grid-template-columns: 148px minmax(0, 1fr) 72px 72px 88px minmax(0, 132px);
  column-gap: 8px;
  padding: 0;
}

.row > div {
  white-space: nowrap;
}

.result {
  overflow: hidden;
  text-overflow: ellipsis;
}

.more {
  align-self: flex-start;
}

.body .row {
  min-height: 40px;
  border-bottom: 1px solid var(--row-border);
}

.target {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.result.failed {
  color: var(--danger-text);
}
</style>
