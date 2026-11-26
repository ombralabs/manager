<script setup lang="ts">
import { computed, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FieldError, FieldName, Fingerprint, FingerprintOptions, Screen } from '../../api/types'
import type { SelectOption } from '../../components/types'
import UiButton from '../../components/UiButton.vue'
import UiField from '../../components/UiField.vue'
import UiSegmented from '../../components/UiSegmented.vue'
import UiSelect from '../../components/UiSelect.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { useFieldErrors } from '../../lib/errorText'
import { engineMajor, gpuName } from '../../lib/format'
import { markManual, parseCoordinate, parseQuota, parseSeed, unmarkManual } from '../../lib/profileForm'
import ManualBadge from './ManualBadge.vue'

const fp = defineModel<Fingerprint>({ required: true })
const props = defineProps<{ options: FingerprintOptions; errors: FieldError[]; direct?: boolean }>()
const emit = defineEmits<{ regenerate: [] }>()
const { t } = useI18n()
const err = useFieldErrors(() => props.errors)

const isManual = (field: FieldName) => fp.value.manual.includes(field)
const unmark = (field: FieldName) => unmarkManual(fp.value, field)

/** Значение поменял человек — при новой генерации оно сохранится. */
function setByHand<K extends keyof Fingerprint & FieldName>(field: K, value: Fingerprint[K]) {
  fp.value[field] = value
  markManual(fp.value, field)
}

/** Текущее значение всегда есть в списке, даже если его нет в наборе комбинаций. */
function withCurrent(options: SelectOption[], current: SelectOption): SelectOption[] {
  return options.some((option) => option.value === current.value) ? options : [current, ...options]
}

/** Текст поля живёт в модели редактора: пустое или неверное значение не попадает в отпечаток, ошибку даст `clientErrors`. */
const seedText = defineModel<string>('seedText', { required: true })
watch(seedText, (text) => {
  const seed = parseSeed(text)
  if (seed !== null && seed !== fp.value.seed) fp.value.seed = seed
})
// «Сгенерировать заново» меняет seed отпечатка — поле показывает новый.
watch(
  () => fp.value.seed,
  (seed) => {
    if (parseSeed(seedText.value) !== seed) seedText.value = String(seed)
  },
)

const osOptions = computed<SelectOption[]>(() => [
  { value: 'macos', label: 'macOS' },
  { value: 'windows', label: t('editor.fingerprint.windowsUnavailable'), disabled: true },
])
const osModel = computed({ get: () => fp.value.os, set: () => undefined })
const brandOptions: SelectOption[] = [{ value: 'Chrome', label: 'Chrome' }]
const brandModel = computed({ get: () => fp.value.brand, set: () => undefined })
const coreVersion = computed(() => engineMajor(props.options.engine_version) ?? '—')
const osVersionModel = computed({
  get: () => fp.value.os_version,
  set: (value: string) => setByHand('os_version', value.trim()),
})

const screenKey = (screen: Screen) => `${screen.width}x${screen.height}`
const screenLabel = (screen: Screen) => `${screen.width} × ${screen.height}`
const screenOptions = computed(() =>
  withCurrent(
    props.options.screens.map((screen) => ({ value: screenKey(screen), label: screenLabel(screen) })),
    { value: screenKey(fp.value.screen), label: screenLabel(fp.value.screen) },
  ),
)
const screenModel = computed({
  get: () => screenKey(fp.value.screen),
  set: (value: string) => {
    const [width, height] = value.split('x').map(Number)
    setByHand('screen', { width, height })
  },
})

const gpuOptions = computed(() =>
  withCurrent(
    props.options.gpus.map((gpu) => ({ value: gpu.renderer, label: gpuName(gpu.renderer) })),
    { value: fp.value.gpu.renderer, label: gpuName(fp.value.gpu.renderer) },
  ),
)
const gpuModel = computed({
  get: () => fp.value.gpu.renderer,
  set: (renderer: string) => {
    const gpu = props.options.gpus.find((g) => g.renderer === renderer)
    if (gpu) setByHand('gpu', { ...gpu })
  },
})

const numberOptions = (values: number[], current: number) =>
  withCurrent(
    values.map((value) => ({ value: String(value), label: String(value) })),
    { value: String(current), label: String(current) },
  )
const coresOptions = computed(() => numberOptions(props.options.hardware_concurrency, fp.value.hardware_concurrency))
const coresModel = computed({
  get: () => String(fp.value.hardware_concurrency),
  set: (value: string) => setByHand('hardware_concurrency', Number(value)),
})
const memoryOptions = computed(() => numberOptions(props.options.device_memory, fp.value.device_memory))
const memoryModel = computed({
  get: () => String(fp.value.device_memory),
  set: (value: string) => setByHand('device_memory', Number(value)),
})

const modeOptions = computed<SelectOption[]>(() => [
  { value: 'auto', label: t(props.direct ? 'editor.gateway.byConnection' : 'editor.fingerprint.byGateway') },
  { value: 'manual', label: t('editor.fingerprint.manual') },
])

/** «По IP шлюза» снимает пометку «вручную», «Вручную» ставит её и подставляет значение по умолчанию. */
function settingMode(field: 'timezone' | 'language', fallback: () => string) {
  return computed({
    get: () => fp.value[field].mode,
    set: (mode: string) => {
      if (mode === 'auto') {
        fp.value[field] = { mode: 'auto' }
        unmarkManual(fp.value, field)
      } else if (fp.value[field].mode !== 'manual') {
        setByHand(field, { mode: 'manual', value: fallback() })
      }
    },
  })
}

function settingValue(field: 'timezone' | 'language') {
  return computed({
    get: () => {
      const setting = fp.value[field]
      return setting.mode === 'manual' ? setting.value : ''
    },
    set: (value: string) => setByHand(field, { mode: 'manual', value: value.trim() }),
  })
}

const timezoneMode = settingMode('timezone', () => Intl.DateTimeFormat().resolvedOptions().timeZone)
const timezoneValue = settingValue('timezone')
const languageMode = settingMode('language', () => 'en-US')
const languageValue = settingValue('language')

const webrtcOptions = computed<SelectOption[]>(() => [
  { value: 'auto', label: t(props.direct ? 'editor.gateway.connectionIp' : 'editor.fingerprint.gatewayIp') },
  { value: 'manual', label: t('editor.fingerprint.ownIp') },
])
const webrtcMode = computed({
  get: () => fp.value.webrtc.mode,
  set: (mode: string) => {
    if (mode === 'auto') {
      fp.value.webrtc = { mode: 'auto' }
      unmarkManual(fp.value, 'webrtc')
    } else if (fp.value.webrtc.mode !== 'manual') {
      setByHand('webrtc', { mode: 'manual', ip: '' })
    }
  },
})
const webrtcIp = computed({
  get: () => {
    const webrtc = fp.value.webrtc
    return webrtc.mode === 'manual' ? webrtc.ip : ''
  },
  set: (ip: string) => setByHand('webrtc', { mode: 'manual', ip: ip.trim() }),
})

const noiseModel = computed({ get: () => fp.value.noise, set: (on: boolean) => setByHand('noise', on) })

const dprOptions: SelectOption[] = [1, 2, 3].map((value) => ({ value: String(value), label: `${value}×` }))
const dprModel = computed({
  get: () => String(fp.value.device_pixel_ratio),
  set: (value: string) => setByHand('device_pixel_ratio', Number(value)),
})
const touchOptions: SelectOption[] = [0, 5, 10].map((value) => ({ value: String(value), label: String(value) }))
const touchModel = computed({
  get: () => String(fp.value.max_touch_points),
  set: (value: string) => setByHand('max_touch_points', Number(value)),
})

/** Текст поля — в модели редактора, как seed: верное число или пусто сразу попадают в отпечаток, неверный текст остаётся в поле и ждёт `clientErrors`. */
const quotaText = defineModel<string>('quotaText', { required: true })
const quotaModel = computed({
  get: () => quotaText.value,
  set: (text: string) => {
    quotaText.value = text
    if (text.trim() === '') {
      setByHand('storage_quota_mb', null)
      return
    }
    const value = parseQuota(text)
    if (value !== null) setByHand('storage_quota_mb', value)
  },
})
// «Сгенерировать заново» может сменить квоту без пометки «вручную» — поле показывает новую.
watch(
  () => fp.value.storage_quota_mb,
  (value) => {
    const shown = quotaText.value.trim() === '' ? null : parseQuota(quotaText.value)
    if (shown !== value) quotaText.value = value === null ? '' : String(value)
  },
)

/** Тексты полей координат живут в модели редактора, как seed. */
const latText = defineModel<string>('latText', { required: true })
const lonText = defineModel<string>('lonText', { required: true })

const locationOptions = computed<SelectOption[]>(() => [
  { value: 'off', label: t('editor.fingerprint.locationOff') },
  { value: 'manual', label: t('editor.fingerprint.manual') },
])
/** «Вручную» ставит пометку и пустые поля (сохранение без чисел остановит `clientErrors`); «Не задана» снимает. */
const locationMode = computed({
  get: () => fp.value.location.mode,
  set: (mode: string) => {
    if (mode === 'off') {
      fp.value.location = { mode: 'off' }
      unmarkManual(fp.value, 'location')
      latText.value = lonText.value = ''
    } else if (fp.value.location.mode !== 'manual') {
      setByHand('location', { mode: 'manual', lat: 0, lon: 0 })
      latText.value = lonText.value = ''
    }
  },
})
watch([latText, lonText], ([lat, lon]) => {
  const current = fp.value.location
  if (current.mode !== 'manual') return
  const parsedLat = parseCoordinate(lat, 90)
  const parsedLon = parseCoordinate(lon, 180)
  if (parsedLat === null || parsedLon === null) return
  if (parsedLat !== current.lat || parsedLon !== current.lon) {
    setByHand('location', { mode: 'manual', lat: parsedLat, lon: parsedLon })
  }
})
</script>

<template>
  <div class="stack fingerprint-form">
    <section class="card section split">
      <div class="intro">
        <h3 class="section-title">{{ t('editor.fingerprint.autogen') }}</h3>
        <div class="hint">{{ t('editor.fingerprint.autogenText') }}</div>
      </div>
      <div class="autogen">
        <UiField :label="t('editor.fingerprint.seed')" input-id="seed" :hint="t('editor.fingerprint.seedHint')" :error="err('seed')">
          <input id="seed" v-model="seedText" class="input mono seed" :class="{ invalid: err('seed') }" inputmode="numeric" />
        </UiField>
        <UiButton icon="refresh" @click="emit('regenerate')">{{ t('editor.fingerprint.regenerate') }}</UiButton>
      </div>
    </section>

    <section class="card section">
      <h3 class="section-title">{{ t('editor.fingerprint.system') }}</h3>
      <div class="grid four">
        <UiField :label="t('editor.fingerprint.os')" input-id="os" :error="err('os')">
          <UiSelect id="os" v-model="osModel" :options="osOptions" />
        </UiField>
        <UiField :label="t('editor.fingerprint.osVersion')" input-id="os-version" :error="err('os_version')">
          <template #badge><ManualBadge v-if="isManual('os_version')" @clear="unmark('os_version')" /></template>
          <input id="os-version" v-model.lazy="osVersionModel" class="input" :class="{ invalid: err('os_version') }" />
        </UiField>
        <UiField :label="t('editor.fingerprint.brand')" input-id="brand" :error="err('brand')">
          <UiSelect id="brand" v-model="brandModel" :options="brandOptions" disabled />
        </UiField>
        <UiField :label="t('editor.fingerprint.coreVersion')" input-id="core">
          <input id="core" class="input" :value="coreVersion" disabled />
        </UiField>
      </div>
    </section>

    <section class="card section">
      <h3 class="section-title">{{ t('editor.fingerprint.device') }}</h3>
      <div class="grid four">
        <UiField :label="t('editor.fingerprint.screen')" input-id="screen" :error="err('screen')">
          <template #badge><ManualBadge v-if="isManual('screen')" @clear="unmark('screen')" /></template>
          <UiSelect id="screen" v-model="screenModel" :options="screenOptions" />
        </UiField>
        <UiField :label="t('editor.fingerprint.gpu')" input-id="gpu" :error="err('gpu')">
          <template #badge><ManualBadge v-if="isManual('gpu')" @clear="unmark('gpu')" /></template>
          <UiSelect id="gpu" v-model="gpuModel" :options="gpuOptions" />
        </UiField>
        <UiField :label="t('editor.fingerprint.cores')" input-id="cores" :error="err('hardware_concurrency')">
          <template #badge>
            <ManualBadge v-if="isManual('hardware_concurrency')" @clear="unmark('hardware_concurrency')" />
          </template>
          <UiSelect id="cores" v-model="coresModel" :options="coresOptions" />
        </UiField>
        <UiField :label="t('editor.fingerprint.memory')" input-id="memory" :error="err('device_memory')">
          <template #badge><ManualBadge v-if="isManual('device_memory')" @clear="unmark('device_memory')" /></template>
          <UiSelect id="memory" v-model="memoryModel" :options="memoryOptions" />
        </UiField>
        <UiField :label="t('editor.fingerprint.dpr')" input-id="dpr" :error="err('device_pixel_ratio')">
          <template #badge>
            <ManualBadge v-if="isManual('device_pixel_ratio')" @clear="unmark('device_pixel_ratio')" />
          </template>
          <UiSelect id="dpr" v-model="dprModel" :options="dprOptions" />
        </UiField>
        <UiField :label="t('editor.fingerprint.touchPoints')" input-id="touch-points" :error="err('max_touch_points')">
          <template #badge>
            <ManualBadge v-if="isManual('max_touch_points')" @clear="unmark('max_touch_points')" />
          </template>
          <UiSelect id="touch-points" v-model="touchModel" :options="touchOptions" />
        </UiField>
        <UiField
          :label="t('editor.fingerprint.storageQuota')"
          input-id="storage-quota"
          :hint="t('editor.fingerprint.storageQuotaHint')"
          :error="err('storage_quota_mb')"
        >
          <template #badge>
            <ManualBadge v-if="isManual('storage_quota_mb')" @clear="unmark('storage_quota_mb')" />
          </template>
          <input
            id="storage-quota"
            v-model="quotaModel"
            class="input mono"
            :class="{ invalid: err('storage_quota_mb') }"
            inputmode="numeric"
            spellcheck="false"
          />
        </UiField>
      </div>
    </section>

    <section class="card section">
      <h3 class="section-title">{{ t('editor.fingerprint.network') }}</h3>
      <div class="grid three">
        <UiField :label="t('editor.fingerprint.timezone')" :error="err('timezone')">
          <template #badge><ManualBadge v-if="isManual('timezone')" @clear="unmark('timezone')" /></template>
          <UiSegmented v-model="timezoneMode" :options="modeOptions" :aria-label="t('editor.fingerprint.timezone')" />
          <input
            v-if="timezoneMode === 'manual'"
            v-model.lazy="timezoneValue"
            class="input mono"
            :class="{ invalid: err('timezone') }"
            :aria-label="t('editor.fingerprint.timezone')"
            placeholder="Europe/Berlin"
            spellcheck="false"
          />
        </UiField>
        <UiField :label="t('editor.fingerprint.language')" :error="err('language')">
          <template #badge><ManualBadge v-if="isManual('language')" @clear="unmark('language')" /></template>
          <UiSegmented v-model="languageMode" :options="modeOptions" :aria-label="t('editor.fingerprint.language')" />
          <input
            v-if="languageMode === 'manual'"
            v-model.lazy="languageValue"
            class="input mono"
            :class="{ invalid: err('language') }"
            :aria-label="t('editor.fingerprint.language')"
            placeholder="de-DE"
            spellcheck="false"
          />
        </UiField>
        <UiField :label="t('editor.fingerprint.webrtc')" :error="err('webrtc')">
          <template #badge><ManualBadge v-if="isManual('webrtc')" @clear="unmark('webrtc')" /></template>
          <UiSegmented v-model="webrtcMode" :options="webrtcOptions" :aria-label="t('editor.fingerprint.webrtc')" />
          <input
            v-if="webrtcMode === 'manual'"
            v-model.lazy="webrtcIp"
            class="input mono"
            :class="{ invalid: err('webrtc') }"
            :aria-label="t('editor.fingerprint.webrtc')"
            placeholder="203.0.113.24"
            spellcheck="false"
          />
        </UiField>
      </div>
    </section>

    <section class="card section">
      <h3 class="section-title">{{ t('editor.fingerprint.location') }}</h3>
      <div class="grid three">
        <UiField :label="t('editor.fingerprint.location')" :error="err('location')">
          <template #badge><ManualBadge v-if="isManual('location')" @clear="unmark('location')" /></template>
          <UiSegmented v-model="locationMode" :options="locationOptions" :aria-label="t('editor.fingerprint.location')" />
        </UiField>
      </div>
      <div v-if="locationMode === 'manual'" class="grid two">
        <UiField :label="t('editor.fingerprint.latitude')" input-id="latitude" :error="err('location.lat')">
          <input
            id="latitude"
            v-model="latText"
            class="input mono"
            :class="{ invalid: err('location.lat') }"
            inputmode="decimal"
            placeholder="55.7558"
            spellcheck="false"
          />
        </UiField>
        <UiField :label="t('editor.fingerprint.longitude')" input-id="longitude" :error="err('location.lon')">
          <input
            id="longitude"
            v-model="lonText"
            class="input mono"
            :class="{ invalid: err('location.lon') }"
            inputmode="decimal"
            placeholder="37.6173"
            spellcheck="false"
          />
        </UiField>
      </div>
      <div class="hint">{{ t('editor.fingerprint.locationHint') }}</div>
    </section>

    <section class="card section noise-section">
      <div class="split">
        <div class="intro">
          <h3 class="section-title">{{ t('editor.workspace.noiseTitle') }}</h3>
          <div class="hint">{{ t('editor.fingerprint.noiseText') }}</div>
        </div>
        <UiSwitch v-model="noiseModel" :label="t('editor.fingerprint.noise')" />
      </div>
      <div class="noise-surfaces"><div v-for="surface in ['Canvas', 'WebGL', 'Audio', 'ClientRects']" :key="surface"><span>{{ surface }}</span><span :class="{ enabled: fp.noise }">{{ t(fp.noise ? 'editor.workspace.noiseOn' : 'editor.workspace.noiseOff') }}</span></div></div>
    </section>
  </div>
</template>

<style scoped>
.stack {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.split {
  display: flex;
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
}

.intro {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.autogen {
  display: flex;
  align-items: flex-end;
  gap: 10px;
  flex-shrink: 0;
}

.seed {
  width: 108px;
}
.fingerprint-form :deep(.grid) { display: flex; flex-direction: column; gap: 0; }
.fingerprint-form :deep(.grid > .field) { display: grid; grid-template-columns: 160px minmax(0, 1fr); grid-template-rows: auto; align-items: start; gap: 16px; padding: 16px 0; border-bottom: 1px solid var(--row-border); }
.fingerprint-form :deep(.grid > .field:last-child) { padding-bottom: 0; border-bottom: 0; }
.fingerprint-form :deep(.grid > .field > .head) { grid-row: 1; align-self: start; align-items: center; min-height: 44px; flex-wrap: wrap; justify-content: flex-start; gap: 6px; }
.fingerprint-form :deep(.grid > .field > .body) { grid-column: 2; grid-row: 1; }
.noise-surfaces { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; margin-top: 4px; }
.noise-surfaces > div { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 10px 12px; border: 1px solid var(--border); border-radius: 6px; font-size: 12px; }
.noise-surfaces div > span:last-child { color: var(--text-2); font-size: 11px; }
.noise-surfaces div > span.enabled { color: var(--accent-ink); }
@media (max-width: 1280px) {
  .split { flex-direction: column; align-items: stretch; gap: 16px; }
  .autogen { flex-wrap: wrap; }
  .noise-section .split { flex-direction: row; align-items: center; }
  .fingerprint-form :deep(.grid > .field) { grid-template-columns: 132px minmax(0, 1fr); }
}
@media (max-width: 600px) {
  .fingerprint-form :deep(.grid > .field) { grid-template-columns: minmax(0, 1fr); gap: 6px; }
  .fingerprint-form :deep(.grid > .field > .head) { min-height: 0; }
  .fingerprint-form :deep(.grid > .field > .body) { grid-column: 1; grid-row: 2; }
}
</style>
