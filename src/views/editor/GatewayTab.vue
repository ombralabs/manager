<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { ConnectionCheck, FieldError, GeoInfo, ProxyRecord } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import ProxyDialog from '../../components/ProxyDialog.vue'
import SecretField from '../../components/SecretField.vue'
import UiButton from '../../components/UiButton.vue'
import UiField from '../../components/UiField.vue'
import UiDetails from '../../components/UiDetails.vue'
import UiSegmented from '../../components/UiSegmented.vue'
import UiSelect from '../../components/UiSelect.vue'
import { formatDateTime } from '../../lib/columns'
import { useFieldErrors } from '../../lib/errorText'
import { kindLabel } from '../../lib/format'
import { fieldsOrToast, notify } from '../../lib/notify'
import { CHECK_ICON, checkState, countryIp, draftFromRow, proxyAddress, proxyDraftErrors, toProxyEditForm } from '../../lib/proxies'
import { kindOptions, proxyOptions } from '../../lib/proxyOptions'
import type { EditorModel } from '../../lib/profileForm'
import { useProxiesStore } from '../../stores/proxies'

const model = defineModel<EditorModel>({ required: true })
const props = defineProps<{ profileId: string | null; running: boolean; replacedAt: number | null; errors: FieldError[] }>()
const { t, locale } = useI18n()
const proxies = useProxiesStore()
const dialogOpen = ref(false)
const checking = ref(false)
const result = ref<GeoInfo | null>(null)
const checkErrors = ref<FieldError[]>([])
const err = useFieldErrors(() => [...props.errors, ...checkErrors.value])
const pinned = computed(() => proxies.rows.find((row) => row.id === model.value.proxyId) ?? null)
const isSaved = computed(() => model.value.proxyId === model.value.savedProxyId)
const mode = computed({
  get: () => model.value.connectionMode,
  set: (value: 'direct' | 'manual' | 'saved') => {
    if (value === 'manual' && pinned.value && (model.value.manualProxyId !== pinned.value.id || !model.value.proxyDraft.host)) {
      model.value.proxyDraft = draftFromRow(pinned.value)
      model.value.manualProxyId = pinned.value.id
    }
    if (value === 'manual') model.value.proxyDraft.enabled = true
    model.value.connectionMode = value
    model.value.network.direct = value === 'direct'
  },
})
const modeOptions = computed(() => [
  { value: 'direct', label: t('editor.gateway.noProxy') },
  { value: 'manual', label: t('editor.gateway.configure') },
  { value: 'saved', label: t('editor.gateway.savedProxies') },
])
const checkerOptions = computed(() => [
  { value: 'auto', label: t('editor.gateway.checkerAuto') },
  { value: 'ipwhois', label: 'ipwho.is' },
  { value: 'ipapi', label: 'ipapi.is' },
  { value: 'ip2location', label: 'IP2Location' },
  { value: 'ip-api', label: 'ip-api' },
  { value: 'unavailable-ipfoxy', label: t('editor.gateway.ipfoxyUnavailable'), disabled: true, title: t('editor.gateway.ipfoxyHint') },
])
const choices = computed(() => proxyOptions(proxies.rows, t('editor.gateway.random'), t('editor.gateway.off'), (name) => t('editor.gateway.busy', { name }), props.profileId))
const selected = computed({ get: () => model.value.proxyId === null ? '' : String(model.value.proxyId), set: (value: string) => { model.value.proxyId = value === '' ? null : Number(value) } })
const checkLine = computed(() => {
  if (!pinned.value) return ''
  const state = t(`editor.gateway.state.${checkState(pinned.value)}`)
  return pinned.value.check ? t('editor.gateway.checkedAt', { state, at: formatDateTime(pinned.value.check.at, locale.value) }) : state
})
const target = (): ConnectionCheck | null => mode.value === 'direct' ? { mode: 'direct' }
  : mode.value === 'manual' ? { mode: 'manual', proxy: toProxyEditForm(model.value.proxyDraft), source_id: model.value.manualProxyId }
    : model.value.proxyId === null ? null : { mode: 'saved', id: model.value.proxyId }
let version = 0
let mounted = true
watch(() => JSON.stringify([model.value.connectionMode, model.value.proxyId, model.value.proxyDraft, model.value.network.ip_checker]), () => {
  version++; result.value = null; checkErrors.value = []
})
onBeforeUnmount(() => { mounted = false; version++ })
async function check() {
  if (checking.value) return
  checkErrors.value = mode.value === 'manual' ? proxyDraftErrors(model.value.proxyDraft) : []
  const connection = target()
  if (!connection || checkErrors.value.length) return
  const request = ++version
  checking.value = true; result.value = null
  try {
    const checked = await core.checkConnection(connection, model.value.network.ip_checker)
    if (mounted && request === version) result.value = checked
    if (connection.mode === 'saved') await proxies.refresh().catch((e: unknown) => notify.error(e))
  } catch (e) {
    if (mounted && request === version) fieldsOrToast(e, checkErrors)
  } finally { if (mounted) checking.value = false }
}
function added(record: ProxyRecord) {
  model.value.proxyId = record.id
  mode.value = 'saved'
  proxies.refresh().catch((e: unknown) => notify.error(e))
}
</script>

<template>
  <div class="stack">
    <fieldset class="card section proxy-configuration" :disabled="checking">
      <h3 class="section-title">{{ t('editor.gateway.title') }}</h3>
      <UiSegmented v-model="mode" :options="modeOptions" :aria-label="t('editor.gateway.connectionMode')" />
      <div v-if="running" class="notice">{{ t('editor.gateway.nextLaunch') }}</div>
      <div v-if="replacedAt !== null && isSaved && mode === 'saved'" class="notice">{{ t('editor.gateway.replaced') }}</div>
      <div v-if="mode === 'direct'" class="direct-connection"><AppIcon name="external" :size="22" /><div><strong>{{ t('editor.gateway.noProxy') }}</strong><p>{{ t('editor.gateway.directHint') }}</p></div></div>
      <template v-else-if="mode === 'manual'">
        <p class="hint">{{ t('editor.gateway.manualHint') }}</p>
        <UiField :label="t('proxyDialog.kind')"><UiSegmented v-model="model.proxyDraft.kind" :options="kindOptions" :aria-label="t('proxyDialog.kind')" /></UiField>
        <div class="address">
          <UiField :label="t('proxyDialog.host')" input-id="profile-proxy-host" :hint="t('proxyDialog.hostHint')" :error="err('proxy_host')"><input id="profile-proxy-host" v-model="model.proxyDraft.host" class="input mono" :class="{ invalid: err('proxy_host') }" placeholder="proxy.example.com" autocomplete="off" spellcheck="false" /></UiField>
          <UiField :label="t('proxyDialog.port')" input-id="profile-proxy-port" :error="err('proxy_port')"><input id="profile-proxy-port" v-model="model.proxyDraft.port" class="input mono" :class="{ invalid: err('proxy_port') }" placeholder="1080" inputmode="numeric" /></UiField>
        </div>
        <div class="grid two">
          <UiField :label="t('proxyDialog.username')" input-id="profile-proxy-username" :error="err('proxy_username')"><input id="profile-proxy-username" v-model="model.proxyDraft.username" class="input" autocomplete="off" /></UiField>
          <UiField :label="t('proxyDialog.password')" input-id="profile-proxy-password" :error="err('proxy_password')"><SecretField v-model="model.proxyDraft.password" input-id="profile-proxy-password" :invalid="!!err('proxy_password')" /></UiField>
        </div>
      </template>
      <template v-else>
        <div class="choose">
          <UiField :label="t('editor.gateway.proxy')" input-id="gateway-proxy" :hint="!proxies.rows.length ? t('editor.gateway.empty') : model.proxyId === null ? t('editor.gateway.randomHint') : undefined" :error="err('proxy_id')"><UiSelect id="gateway-proxy" v-model="selected" :options="choices" :invalid="!!err('proxy_id')" /></UiField>
          <UiButton class="add" icon="plus" @click="dialogOpen = true">{{ t('editor.gateway.add') }}</UiButton>
        </div>
        <div v-if="pinned" class="saved-proxy">
          <div class="pinned-head"><div><h3 class="section-title">{{ t(isSaved ? 'editor.gateway.pinned' : 'editor.gateway.selected') }}</h3><div class="mono">{{ kindLabel(pinned.kind) }} · {{ proxyAddress(pinned) }}</div></div><button type="button" class="link" @click="model.proxyId = null">{{ t('editor.gateway.reset') }}</button></div>
          <dl class="facts"><div><dt>{{ t('editor.gateway.countryIp') }}</dt><dd class="mono">{{ countryIp(pinned) ?? t('editor.gateway.unknown') }}</dd></div><div><dt>{{ t('editor.gateway.lastCheck') }}</dt><dd :class="['state', checkState(pinned)]"><AppIcon :name="CHECK_ICON[checkState(pinned)]" :size="16" />{{ checkLine }}</dd></div></dl>
        </div>
      </template>
    </fieldset>
    <section class="card section">
      <h3 class="section-title">{{ t('editor.gateway.connectionCheck') }}</h3>
      <div class="check-controls"><UiField :label="t('editor.gateway.ipChecker')" input-id="profile-ip-checker"><UiSelect id="profile-ip-checker" v-model="model.network.ip_checker" :options="checkerOptions" :disabled="checking" /></UiField><UiButton icon="refresh" :loading="checking" :disabled="mode === 'saved' && model.proxyId === null" @click="check">{{ t('editor.gateway.checkNetwork') }}</UiButton></div>
      <p class="hint">{{ t(model.network.ip_checker === 'auto' ? 'editor.gateway.autoCheckerHint' : 'editor.gateway.selectedCheckerHint') }}</p>
      <UiDetails v-if="model.network.ip_checker === 'auto'" :text="t('editor.gateway.autoCheckerDetails')" />
      <p v-if="model.network.ip_checker === 'ip-api'" class="hint">{{ t('editor.gateway.ipApiHttpHint') }}</p>
      <div v-if="result" class="connection-result" role="status"><div class="result-heading"><AppIcon name="tick" :size="18" />{{ t('editor.gateway.connectionOk') }}</div><dl class="facts"><div><dt>IP</dt><dd class="mono">{{ result.ip }}</dd></div><div><dt>{{ t('editor.gateway.country') }}</dt><dd>{{ result.country }}</dd></div><div><dt>{{ t('editor.fingerprint.timezone') }}</dt><dd>{{ result.timezone }}</dd></div></dl></div>
    </section>
    <ProxyDialog v-model:open="dialogOpen" :proxy="null" @saved="added" />
  </div>
</template>

<style scoped>
.stack { display: flex; flex-direction: column; gap: 16px; }
.proxy-configuration { min-width: 0; margin: 0; }
.proxy-configuration > :deep(.segmented) { align-self: flex-start; }
.address { display: grid; grid-template-columns: minmax(0, 1fr) 120px; gap: 16px; }
.choose { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: start; gap: 12px; }
.add { margin-top: 24px; }
.direct-connection { display: flex; align-items: center; gap: 12px; padding: 18px; border: 1px solid var(--border); border-radius: var(--radius-control); background: var(--muted-bg); }
.direct-connection > .icon { color: var(--accent-ink); }
.direct-connection strong { font-size: 13px; font-weight: 500; }
.direct-connection p { margin: 5px 0 0; color: var(--text-2); font-size: 12px; line-height: 18px; }
.hint { margin: 0; }
.saved-proxy { display: flex; flex-direction: column; gap: 18px; margin-top: 6px; padding-top: 20px; border-top: 1px solid var(--border); }
.pinned-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; }
.pinned-head .mono { margin-top: 7px; font-size: 12px; overflow-wrap: anywhere; }
.pinned-head .link { font-size: 12px; font-weight: 400; }
.check-controls { display: grid; grid-template-columns: minmax(180px, 1fr) auto; align-items: end; gap: 12px; }
.facts { display: flex; flex-wrap: wrap; gap: 20px; margin: 0; font-size: 12px; }
.facts > div { flex: 1 1 100px; }
.facts dt { color: var(--text-2); }
.facts dd { margin: 5px 0 0; overflow-wrap: anywhere; }
.state { display: flex; align-items: center; gap: 6px; }
.state.ok { color: var(--ok-text); }
.state.failed { color: var(--danger-text); }
.connection-result { display: flex; flex-direction: column; gap: 16px; padding: 16px; border: 1px solid var(--border); border-radius: var(--radius-control); }
.result-heading { display: flex; align-items: center; gap: 7px; color: var(--ok-text); font-size: 13px; font-weight: 500; }
@media (max-width: 1150px) { .choose, .check-controls { grid-template-columns: minmax(0, 1fr); } .add { margin-top: 0; } .check-controls > .btn { justify-self: start; } }
</style>
