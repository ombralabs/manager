<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FieldError } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import UiButton from '../../components/UiButton.vue'
import { setLabel } from '../../lib/extensions'
import { engineMajor, gpuName, kindLabel } from '../../lib/format'
import { countryIp, proxyAddress } from '../../lib/proxies'
import type { EditorModel, EditorTab } from '../../lib/profileForm'
import { useExtensionsStore } from '../../stores/extensions'
import { useProxiesStore } from '../../stores/proxies'
import { useGroupsStore } from '../../stores/groups'

const props = defineProps<{ model: EditorModel; errors: FieldError[]; engineVersion: string | null; profileId?: string | null; busy?: boolean }>()
const emit = defineEmits<{ openTab: [tab: EditorTab]; regenerate: [] }>()
const { t } = useI18n()
const proxies = useProxiesStore()
const extensions = useExtensionsStore()
const groups = useGroupsStore()
const groupName = computed(() => props.model.group === null ? t('common.ungrouped') : groups.byId(props.model.group)?.name ?? '—')
/** «Работа (3)»; список не загружен или набор удалён — «—». */
const extSummary = computed(() => {
  const set = extensions.categoryById(props.model.extCategoryId)
  return set ? `${setLabel(set)} (${set.extension_ids.length})` : '—'
})

const FIELD_LABELS: Record<string, string> = {
  seed: 'editor.fingerprint.seed',
  os: 'editor.fingerprint.os',
  os_version: 'editor.fingerprint.osVersion',
  brand: 'editor.fingerprint.brand',
  gpu: 'editor.fingerprint.gpu',
  hardware_concurrency: 'editor.fingerprint.cores',
  device_memory: 'editor.fingerprint.memory',
  screen: 'editor.fingerprint.screen',
  timezone: 'editor.fingerprint.timezone',
  language: 'editor.fingerprint.language',
  webrtc: 'editor.fingerprint.webrtc',
  device_pixel_ratio: 'editor.fingerprint.dpr',
  max_touch_points: 'editor.fingerprint.touchPoints',
  storage_quota_mb: 'editor.fingerprint.storageQuota',
  location: 'editor.fingerprint.location',
  'location.lat': 'editor.fingerprint.latitude',
  'location.lon': 'editor.fingerprint.longitude',
}

const fp = computed(() => props.model.fingerprint)
const browser = computed(() => {
  const major = engineMajor(props.engineVersion)
  return major ? `${fp.value.brand} ${major}` : fp.value.brand
})
const tzLang = computed(() => {
  const { timezone, language } = fp.value
  const byGateway = t(props.model.connectionMode === 'direct' ? 'editor.gateway.byConnection' : 'editor.summary.byGateway')
  if (timezone.mode === 'auto' && language.mode === 'auto') return byGateway
  const tz = timezone.mode === 'manual' ? timezone.value : byGateway
  const lang = language.mode === 'manual' ? language.value : byGateway
  return `${tz} · ${lang}`
})
const webrtc = computed(() => {
  const setting = fp.value.webrtc
  return setting.mode === 'manual' ? setting.ip : t(props.model.connectionMode === 'direct' ? 'editor.gateway.connectionIp' : 'editor.fingerprint.gatewayIp')
})
const pinned = computed(() => proxies.rows.find((row) => row.id === props.model.proxyId) ?? null)
/** Подпись выбранного прокси; список ещё не загружен — номер записи. `null` — «Случайный при первом запуске». */
const gateway = computed(() => {
  if (props.model.connectionMode === 'direct') return t('editor.gateway.noProxy')
  if (props.model.connectionMode === 'manual') {
    const proxy = props.model.proxyDraft
    return proxy.host.trim() ? `${kindLabel(proxy.kind)} · ${proxy.host.trim()}:${proxy.port}` : t('editor.gateway.configure')
  }
  if (pinned.value) return `${kindLabel(pinned.value.kind)} · ${proxyAddress(pinned.value)}`
  return props.model.proxyId === null ? null : `#${props.model.proxyId}`
})
const gatewayGeo = computed(() => (props.model.connectionMode === 'saved' && pinned.value ? countryIp(pinned.value) : null))
const label = (field: string) => (FIELD_LABELS[field] ? t(FIELD_LABELS[field]) : field)
</script>

<template>
  <aside class="summary card">
    <header class="summary-head"><h2 class="section-title">{{ t('editor.summary.title') }}</h2><UiButton variant="ghost" size="icon" icon="refresh" :icon-size="16" :disabled="busy" :aria-label="t('editor.fingerprint.regenerate')" :title="t('editor.fingerprint.regenerate')" @click="emit('regenerate')" /></header>
    <div class="summary-identity"><span class="fingerprint-mark"><AppIcon name="fingerprint" :size="25" /></span><div><strong>{{ model.name || t('editor.newTitle') }}</strong><span v-if="profileId" class="mono">#{{ profileId }}</span><span v-else>{{ t('editor.workspace.draft') }}</span></div></div>
    <dl class="profile-facts rows">
      <div><dt>{{ t('editor.basic.group') }}</dt><dd>{{ groupName }}</dd></div>
      <div v-if="model.tags.length"><dt>{{ t('editor.basic.tags') }}</dt><dd class="summary-tags"><span v-for="tag in model.tags" :key="tag">{{ tag }}</span></dd></div>
      <div><dt>{{ t('editor.fingerprint.seed') }}</dt><dd class="mono">{{ model.seedText }}</dd></div>
    </dl>
    <div class="divider" />
    <div v-if="errors.length === 0" class="state ok"><AppIcon name="tick" />{{ t('editor.summary.ok') }}</div>
    <div v-else class="state bad">
      <AppIcon name="alert" />
      <div>
        <div>{{ t('editor.summary.issues') }}</div>
        <ul class="issues">
          <li v-for="error in errors" :key="`${error.field}.${error.code}`">
            {{ label(error.field) }}: {{ t(`validation.${error.code}`) }}
          </li>
        </ul>
      </div>
    </div>
    <dl class="rows">
      <div><dt>{{ t('editor.summary.os') }}</dt><dd>macOS {{ fp.os_version }}</dd></div>
      <div><dt>{{ t('editor.summary.browser') }}</dt><dd>{{ browser }}</dd></div>
      <div><dt>{{ t('editor.summary.screen') }}</dt><dd>{{ fp.screen.width }} × {{ fp.screen.height }} @{{ fp.device_pixel_ratio }}x</dd></div>
      <div v-if="fp.location.mode === 'manual' && model.latText.trim() && model.lonText.trim()">
        <dt>{{ t('editor.summary.location') }}</dt>
        <dd class="mono">{{ model.latText.trim() }}, {{ model.lonText.trim() }}</dd>
      </div>
      <div><dt>{{ t('editor.summary.gpu') }}</dt><dd>{{ gpuName(fp.gpu.renderer) }}</dd></div>
      <div>
        <dt>{{ t('editor.summary.coresMemory') }}</dt>
        <dd>{{ t('editor.summary.coresMemoryValue', { cores: fp.hardware_concurrency, memory: fp.device_memory }) }}</dd>
      </div>
      <div><dt>{{ t('editor.summary.tzLang') }}</dt><dd>{{ tzLang }}</dd></div>
      <div><dt>{{ t('editor.summary.webrtc') }}</dt><dd>{{ webrtc }}</dd></div>
      <div><dt>{{ t('editor.summary.noise') }}</dt><dd>{{ fp.noise ? t('editor.summary.on') : t('editor.summary.off') }}</dd></div>
      <div><dt>{{ t('editor.summary.extensions') }}</dt><dd>{{ extSummary }}</dd></div>
    </dl>
    <div class="divider" />
    <div class="gateway">
      <div class="gateway-label">{{ t('editor.summary.gateway') }}</div>
      <template v-if="gateway">
        <div class="mono">{{ gateway }}</div>
        <div v-if="gatewayGeo" class="hint mono">{{ gatewayGeo }}</div>
      </template>
      <div v-else class="hint">{{ t('editor.summary.randomGateway') }}</div>
      <button type="button" class="link assign" @click="emit('openTab', 'gateway')">
        {{ gateway ? t('editor.summary.changeGateway') : t('editor.summary.assign') }}
      </button>
    </div>
    <div class="divider" />
    <div class="footnote">{{ t('editor.summary.footnote') }}</div>
  </aside>
</template>

<style scoped>
.summary {
  display: flex;
  flex-direction: column;
  gap: 16px;
  flex-shrink: 0;
  width: 100%;
  padding: 20px;
}
.summary-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.summary-head .btn.square { width: 28px; height: 28px; }
.summary-identity { display: flex; align-items: center; gap: 10px; min-width: 0; }
.fingerprint-mark { display: grid; place-items: center; flex-shrink: 0; width: 42px; height: 42px; border-radius: 10px; background: var(--accent-soft); color: var(--accent-ink); }
.summary-identity > div { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.summary-identity strong { font-size: 13px; font-weight: 600; overflow-wrap: anywhere; }
.summary-identity div > span { color: var(--text-2); font-size: 11px; }
.summary-tags { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: 4px; }
.summary-tags > span { display: inline-flex; padding: 1px 5px; border-radius: 4px; background: var(--muted-bg); color: var(--text-2); font-size: 10px; }

.state {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 12px;
  border-radius: var(--radius-control);
  font-size: 12px;
  font-weight: 600;
}

.state.ok {
  background: var(--ok-bg);
  color: var(--ok-text);
}

.state.bad {
  background: var(--danger-bg);
  color: var(--danger-text);
}

.issues {
  margin: 4px 0 0;
  padding-left: 16px;
  font-weight: 500;
}

.rows {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin: 0;
  font-size: 13px;
  line-height: 18px;
}

.rows div {
  display: flex;
  justify-content: space-between;
  gap: 12px;
}

.rows dt {
  color: var(--text-2);
}

.rows dd {
  margin: 0;
  text-align: right;
  min-width: 0;
  max-width: 65%;
  overflow-wrap: anywhere;
}

.divider {
  height: 1px;
  background: var(--border);
}

.gateway {
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: 13px;
}

.gateway-label {
  font-weight: 600;
}

.assign {
  align-self: flex-start;
  font-size: 13px;
}

.footnote {
  font-size: 12px;
  line-height: 17px;
  color: var(--text-2);
}
</style>
