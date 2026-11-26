<script setup lang="ts">
import { computed, toRefs, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SyncMode } from '../../api/types'
import UiSegmented from '../../components/UiSegmented.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { useFieldErrors } from '../../lib/errorText'
import { fieldsOrToast, notify } from '../../lib/notify'
import { useSettingsDraftsStore } from '../../stores/settingsDrafts'
import { useSettingsSaveStore } from '../../stores/settingsSave'
import { useSyncStore } from '../../stores/sync'

const { t } = useI18n()
const sync = useSyncStore()
const saves = useSettingsSaveStore()
const draft = useSettingsDraftsStore().sync
const { errors, text } = toRefs(draft)
const err = useFieldErrors(() => errors.value)

const modeOptions = computed(() => [
  { value: 'full_copy' as SyncMode, label: t('sync.mode.full_copy') },
  { value: 'varied' as SyncMode, label: t('sync.mode.varied') },
])
const mode = computed({
  get: () => sync.settings.mode,
  set: (value: SyncMode) => {
    sync.saveSettings({ mode: value }).catch((e: unknown) => notify.error(e))
  },
})

const toggles = ['clicks', 'typing', 'scroll', 'mouse'] as const
const toggleModel = (key: (typeof toggles)[number]) =>
  computed({
    get: () => sync.settings[key],
    set: (value: boolean) => {
      sync.saveSettings({ [key]: value }).catch((e: unknown) => notify.error(e))
    },
  })
const models = Object.fromEntries(toggles.map((key) => [key, toggleModel(key)]))

type DelayKey = 'click_delay' | 'typing_delay'
const delays = ['click_delay', 'typing_delay'] as const
/** Поля «от» и «до» хранят текст, пока ядро не приняло значение. */
watch(
  () => sync.settings,
  (s) => {
    // Поле обновляется, только если его значение в настройках сменилось: набранное, но не принятое, не теряется.
    for (const key of delays) {
      const previous = draft.accepted[key]
      if (previous === null || text.value[key].every((value, i) => value.trim() === String(previous[i]))) {
        text.value[key] = [String(s[key][0]), String(s[key][1])]
      }
      draft.accepted[key] = [...s[key]]
    }
  },
  { immediate: true },
)

async function commit(key: DelayKey) {
  const pair = text.value[key].map((v) => Number(v.trim()))
  if (text.value[key].some((v) => v.trim() === '') || pair.some((n) => !Number.isInteger(n) || n < 0)) {
    saves.invalid('sync.' + key)
    errors.value = [...errors.value.filter((e) => e.field !== key), { field: key, code: 'out_of_range' }]
    return
  }
  try {
    await sync.saveSettings({ [key]: pair as [number, number] })
    errors.value = errors.value.filter((e) => e.field !== key)
  } catch (e) {
    fieldsOrToast(e, errors)
  }
}
</script>

<template>
  <section class="card section">
    <h2 class="section-title">{{ t('sync.card.title') }}</h2>
    <div class="field">
      <span class="label">{{ t('sync.card.mode') }}</span>
      <UiSegmented v-model="mode" :options="modeOptions" :aria-label="t('sync.card.mode')" />
      <div class="hint">{{ t(`sync.card.modeHint.${mode}`) }} {{ t('sync.card.nextStart') }}</div>
    </div>
    <div class="field">
      <span class="label">{{ t('sync.card.repeat') }}</span>
      <div v-for="key in toggles" :key="key" class="row">
        <span>{{ t(`sync.card.${key}`) }}</span>
        <UiSwitch v-model="models[key].value" :label="t(`sync.card.${key}`)" />
      </div>
    </div>
    <template v-if="mode === 'varied'">
      <div v-for="key in delays" :key="key" class="field">
        <span class="label">{{ t(`sync.card.${key}`) }}</span>
        <div class="range">
          <input :id="`sync-${key === 'click_delay' ? 'click' : 'typing'}-from`" v-model="text[key][0]" class="input mono num" :class="{ invalid: err(key) }" inputmode="numeric" @input="saves.edited('sync.' + key)" :aria-label="t('sync.card.from')" @change="commit(key)" />
          <span>—</span>
          <input :id="`sync-${key === 'click_delay' ? 'click' : 'typing'}-to`" v-model="text[key][1]" class="input mono num" :class="{ invalid: err(key) }" inputmode="numeric" @input="saves.edited('sync.' + key)" :aria-label="t('sync.card.to')" @change="commit(key)" />
          <span class="hint">{{ t('sync.card.ms') }}</span>
        </div>
        <div v-if="err(key)" class="error-text">{{ err(key) }}</div>
      </div>
    </template>
  </section>
</template>

<style scoped>
.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.label {
  font-size: 13px;
  line-height: 18px;
  color: var(--text-2);
}

.row,
.range {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.row { min-height: 48px; border-bottom: 1px solid var(--row-border); }
.row:last-child { border-bottom: 0; }
.range {
  justify-content: flex-start;
}

.num {
  width: 80px;
  text-align: right;
}
</style>
