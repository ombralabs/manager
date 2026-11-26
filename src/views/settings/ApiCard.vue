<script setup lang="ts">
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { computed, ref, toRefs, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ApiConfigForm } from '../../api/types'
import ConfirmDialog from '../../components/ConfirmDialog.vue'
import UiButton from '../../components/UiButton.vue'
import UiField from '../../components/UiField.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { apiFailureText, useFieldErrors } from '../../lib/errorText'
import { fieldsOrToast, notify } from '../../lib/notify'
import { apiAddress, parsePort } from '../../lib/settings'
import { useSettingsDraftsStore } from '../../stores/settingsDrafts'
import { useSettingsSaveStore } from '../../stores/settingsSave'
import { useSettingsStore } from '../../stores/settings'

/** Ответ из примера холста: так отвечает `browser/start` (2Б). */
const RESPONSE = `{ "code": 0, "msg": "success",
  "data": {
    "ws": { "puppeteer": "ws://127.0.0.1:9222/devtools/browser/…",
            "selenium": "127.0.0.1:9222" },
    "debug_port": "9222", "webdriver": "" } }`

const { t } = useI18n()
const settings = useSettingsStore()
const saves = useSettingsSaveStore()

const draft = useSettingsDraftsStore().api
const { errors, portText } = toRefs(draft)
const err = useFieldErrors(() => errors.value)
const confirmOpen = ref(false)
const regenerating = ref(false)

watch(
  () => settings.api?.port,
  (port) => {
    if (port === undefined) return
    if (draft.acceptedPort === null || portText.value.trim() === String(draft.acceptedPort)) portText.value = String(port)
    draft.acceptedPort = port
  },
  { immediate: true },
)

const address = computed(() => (settings.api ? apiAddress(settings.api.port) : ''))
const request = computed(() => `GET ${address.value}/api/v1/browser/start?user_id=k7f2q9`)
const statusText = computed(() => {
  const status = settings.apiStatus
  if (status.state === 'listening') return t('settings.api.listening', { port: status.port })
  if (status.state === 'failed') {
    return t('settings.api.failed', { reason: apiFailureText(status.code) })
  }
  return t('settings.api.off')
})

/** Ошибка поля — у поля, прочие — уведомлением. `false` — не сохранено. */
async function apply(patch: Partial<ApiConfigForm>): Promise<boolean> {
  try {
    const config = await settings.applyApi(patch)
    // Снимаются ошибки полей этого патча. «Порт занят» у введённого, ещё не принятого порта остаётся, пока порт
    // не сменится; у реального порта (ошибку вернул переключатель) — снимается: удачное сохранение его приняло.
    const realPort = portText.value.trim() === String(config.port)
    errors.value = errors.value.filter((error) => !(error.field in patch) && !(error.field === 'port' && realPort))
    if (realPort) draft.failedPort = null
    return true
  } catch (e) {
    fieldsOrToast(e, errors)
    return false
  }
}

const enabled = computed({
  get: () => settings.api?.enabled ?? false,
  set: (value: boolean) => {
    void apply({ enabled: value })
  },
})
const requireKey = computed({
  get: () => settings.api?.require_key ?? false,
  set: (value: boolean) => {
    void apply({ require_key: value })
  },
})

/**
 * Enter (`retry`) или уход из поля. Тот же порт ничего не шлёт; занятый или чужой диапазон — ошибка у поля.
 * Неудачный текст гасит только уход из поля: Enter — явный повтор. Запросы идут по очереди хранилища.
 */
async function commitPort(retry: boolean) {
  const api = settings.api
  if (!api) return
  const text = portText.value.trim()
  if (text === String(api.port)) {
    errors.value = []
    draft.failedPort = null
    saves.reset('integrations.api.port')
    return
  }
  if (text === draft.pendingPort || (!retry && text === draft.failedPort)) return
  const port = parsePort(text)
  if (port === null) {
    draft.failedPort = text
    saves.invalid('integrations.api.port')
    errors.value = [{ field: 'port', code: 'out_of_range' }]
    return
  }
  draft.pendingPort = text
  try {
    draft.failedPort = (await apply({ port })) ? null : text
  } finally {
    draft.pendingPort = null
  }
}

async function copy(text: string, done: string) {
  try {
    await writeText(text)
    notify.info(done)
  } catch (e) {
    notify.error(e)
  }
}

function copyAddress() {
  void copy(address.value, t('settings.api.addressCopied'))
}

function copyKey() {
  const key = settings.api?.key
  if (key) void copy(key, t('settings.api.keyCopied'))
}

async function regenerate() {
  regenerating.value = true
  try {
    await settings.regenerateKey()
    notify.info(t('settings.api.keyReplaced'))
  } catch (e) {
    notify.error(e)
  } finally {
    regenerating.value = false
    confirmOpen.value = false
  }
}
</script>

<template>
  <section v-if="settings.api" class="card section">
    <div class="row">
      <div class="intro">
        <h2 class="section-title">{{ t('settings.api.title') }}</h2>
        <div class="hint">{{ t('settings.api.text') }}</div>
      </div>
      <UiSwitch id="api-enabled" v-model="enabled" :label="t('settings.api.enabled')" />
    </div>

    <div class="address-row">
      <UiField :label="t('settings.api.address')" input-id="api-address">
        <div class="copy-field">
          <input id="api-address" class="mono" :value="address" readonly />
          <UiButton
            size="icon"
            variant="ghost"
            icon="copy"
            :aria-label="t('settings.api.copyAddress')"
            :title="t('settings.api.copyAddress')"
            @click="copyAddress"
          />
        </div>
      </UiField>
      <UiField :label="t('settings.api.port')" input-id="api-port"
        @input="saves.edited('integrations.api.port')" :error="err('port')">
        <input
          id="api-port"
        @input="saves.edited('integrations.api.port')"
          v-model="portText"
          class="input mono"
          :class="{ invalid: err('port') }"
          inputmode="numeric"
          @keydown.enter="commitPort(true)"
          @blur="commitPort(false)"
        />
      </UiField>
    </div>
    <div :class="['status', settings.apiStatus.state]">
      <span class="dot dot-sm" aria-hidden="true" />{{ statusText }}
    </div>

    <div class="row">
      <div class="intro">
        <div class="subtitle">{{ t('settings.api.key') }}</div>
        <div class="hint">{{ t('settings.api.keyText') }}</div>
      </div>
      <UiSwitch id="api-require-key" v-model="requireKey" :label="t('settings.api.requireKey')" />
    </div>
    <div v-if="err('require_key')" class="error-text">{{ err('require_key') }}</div>
    <div v-if="settings.api.require_key && settings.api.key" class="copy-row">
      <input class="input mono" :value="settings.api.key" readonly :aria-label="t('settings.api.key')" />
      <UiButton @click="copyKey">{{ t('settings.api.copyKey') }}</UiButton>
      <UiButton icon="refresh" @click="confirmOpen = true">{{ t('settings.api.newKey') }}</UiButton>
    </div>
    <template v-else-if="settings.api.require_key">
      <div class="notice">{{ t('settings.api.keyUnreadable') }}</div>
      <div>
        <UiButton icon="refresh" @click="confirmOpen = true">{{ t('settings.api.newKey') }}</UiButton>
      </div>
    </template>

    <details class="api-example">
      <summary>{{ t('settings.api.example') }}</summary>
      <div class="example mono">
      <pre>{{ request }}</pre>
      <pre class="response">{{ RESPONSE }}</pre>
      </div>
    </details>

    <ConfirmDialog
      v-model:open="confirmOpen"
      :title="t('settings.api.newKeyTitle')"
      :description="t('settings.api.newKeyText')"
      :confirm-label="t('settings.api.newKeyConfirm')"
      :busy="regenerating"
      @confirm="regenerate"
    />
  </section>
</template>

<style scoped>
.row {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}

.intro {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.subtitle {
  font-weight: 600;
}

.address-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 120px;
  align-items: start;
  gap: 16px;
}

/* Адрес — поле только для чтения с кнопкой копирования внутри (холст «Настройки»). */
.copy-field {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  height: var(--control-h);
  padding: 0 4px 0 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
  background: var(--bg);
}

.copy-field:focus-within {
  border-color: var(--focus);
  box-shadow: 0 0 0 3px var(--focus-ring);
}

.copy-field input {
  flex-grow: 1;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  font-size: 13px;
}

.copy-row {
  display: flex;
  gap: 10px;
}

.status {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  line-height: 18px;
  color: var(--text-2);
}

.dot {
  background: var(--switch-off);
}

.status.listening .dot {
  background: var(--ok);
}

.status.failed {
  color: var(--danger-text);
}

.status.failed .dot {
  background: var(--danger-text);
}

.api-example summary { min-height: 44px; padding: 12px 0; cursor: pointer; color: var(--text-2); }
.api-example summary:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.example {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px 16px;
  border-radius: var(--radius-control);
  background: var(--sidebar);
  color: var(--sidebar-strong);
  font-size: 12px;
  line-height: 18px;
}

.example pre {
  margin: 0;
  overflow-x: auto;
  font: inherit;
  white-space: pre;
}

.response {
  color: var(--sidebar-muted);
}
@media (max-width: 700px) {
  .address-row { grid-template-columns: minmax(0, 1fr); }
  .copy-row { flex-wrap: wrap; }
}
</style>
