<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { pickGoogleKeyFile } from '../../api/dialogs'
import type { ProxyRow } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiSelect from '../../components/UiSelect.vue'
import { notify } from '../../lib/notify'
import { useSettingsStore } from '../../stores/settings'

const { t } = useI18n()
const settings = useSettingsStore()

const NONE = 'none'
const proxies = ref<ProxyRow[]>([])
const listed = ref(false)

const missing = computed(
  () => listed.value && settings.servicesProxy !== null && !proxies.value.some((p) => p.id === settings.servicesProxy),
)
const broken = computed(() => settings.servicesProxyError !== null)
/** Сохранённое значение негодно (прокси удалён или настройка повреждена): поле пустое, «Нет» становится настоящим выбором. */
const needsChoice = computed(() => missing.value || broken.value)
const options = computed(() => [
  ...(needsChoice.value ? [{ value: '', label: t('settings.automation.choose') }] : []),
  { value: NONE, label: t('settings.automation.none') },
  ...proxies.value.map((p) => ({ value: String(p.id), label: `${p.kind}://${p.host}:${p.port}` })),
])
const model = computed({
  get: () => (needsChoice.value ? '' : settings.servicesProxy === null ? NONE : String(settings.servicesProxy)),
  set: (value: string) => {
    settings.setServicesProxy(value === NONE ? null : Number(value)).catch((e: unknown) => notify.error(e))
  },
})

/** Пока ключ выбирается и загружается, кнопка занята: второй щелчок не открывает второй диалог. */
const loadingKey = ref(false)
async function loadKey() {
  loadingKey.value = true
  try {
    const path = await pickGoogleKeyFile()
    if (path) await settings.setGoogleKey(path)
  } catch (e) {
    notify.error(e)
  } finally {
    loadingKey.value = false
  }
}

function removeKey() {
  settings.clearGoogleKey().catch((e: unknown) => notify.error(e))
}

onMounted(async () => {
  try {
    proxies.value = await core.listProxies()
    listed.value = true
  } catch (e) {
    notify.error(e)
  }
})
</script>

<template>
  <section class="card section">
    <h2 class="section-title">{{ t('settings.automation.title') }}</h2>
    <div class="setting-row">
      <div class="setting-intro">
        <label for="services-proxy">{{ t('settings.automation.proxy') }}</label>
        <p class="hint">{{ t('settings.automation.proxyHint') }}</p>
        <p v-if="broken" class="error-text">{{ t('settings.automation.corrupt') }}</p>
        <p v-else-if="missing" class="error-text">{{ t('settings.automation.deleted') }}</p>
      </div>
      <div class="control"><UiSelect id="services-proxy" v-model="model" :options="options" mono /></div>
    </div>
    <div class="setting-row">
      <div class="setting-intro">
        <label for="google-key">{{ t('settings.automation.google') }}</label>
        <p v-if="settings.googleKeyBroken" class="error-text">{{ t('settings.automation.googleBroken') }}</p>
        <p v-else-if="settings.googleEmail" class="hint">{{ t('settings.automation.googleHint', { email: settings.googleEmail }) }}</p>
        <p v-else class="hint">{{ t('settings.search.googleHint') }}</p>
      </div>
      <div class="control row">
        <UiButton id="google-key" :loading="loadingKey" @click="loadKey">{{ t('settings.automation.googleLoad') }}</UiButton>
        <UiButton v-if="settings.googleEmail || settings.googleKeyBroken" @click="removeKey">{{ t('settings.automation.googleRemove') }}</UiButton>
      </div>
    </div>
  </section>
</template>

<style scoped>
.setting-row + .setting-row { border-top: 1px solid var(--row-border); padding-top: 16px; }
.setting-intro { overflow-wrap: anywhere; }
.row { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
.control { flex: 0 0 280px; width: 280px; max-width: 100%; }
</style>
