<script setup lang="ts">
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { toCoreError } from '../../api/errors'
import UiButton from '../../components/UiButton.vue'
import { errorTitle, fieldErrorText } from '../../lib/errorText'
import { notify } from '../../lib/notify'

defineOptions({ name: 'TotpCode' })
const props = defineProps<{ profileId: string }>()
const { t } = useI18n()

const code = ref<string | null>(null)
const left = ref(0)
const error = ref<string | null>(null)
let timer: ReturnType<typeof setInterval> | undefined
/** Конец окна текущего кода по часам: тики могут запаздывать (сон, скрытое окно), время — нет. */
let expiresAt = 0
/** Не раньше этого времени пробуем снова после ошибки; 0 — на ближайшем тике. */
let retryAt = 0
let loading = false

/** Ошибка — текстом вместо кода, без уведомления: блок виден, пока открыта вкладка. */
async function load() {
  if (loading) return
  loading = true
  try {
    const result = await core.totpCode(props.profileId)
    expiresAt = Date.now() + result.seconds_left * 1000
    left.value = result.seconds_left
    code.value = result.code
    error.value = null
  } catch (e) {
    const err = toCoreError(e)
    code.value = null
    error.value = fieldErrorText(err.fields, 'totp') ?? errorTitle(err.code)
    // Неверный ключ сам не починится — ждём новое 30-секундное окно; сбой связи повторяем на следующем тике.
    retryAt = err.code === 'Validation' ? (Math.floor(Date.now() / 30_000) + 1) * 30_000 : 0
  } finally {
    loading = false
  }
}

/** Остаток считаем от часов; на нуле (или после ошибки, когда пора) — новый запрос. */
function tick() {
  if (code.value) {
    left.value = Math.max(0, Math.ceil((expiresAt - Date.now()) / 1000))
    if (left.value === 0) void load()
  } else if (Date.now() >= retryAt) void load()
}

function onVisible() {
  if (document.visibilityState === 'visible') void load()
}

onMounted(() => {
  void load()
  timer = setInterval(tick, 1000)
  document.addEventListener('visibilitychange', onVisible)
})
onBeforeUnmount(() => {
  clearInterval(timer)
  document.removeEventListener('visibilitychange', onVisible)
})

const grouped = computed(() => code.value?.replace(/(\d{3})(?=\d)/g, '$1 ') ?? '')

async function copy() {
  if (!code.value) return
  try {
    await writeText(code.value)
    notify.info(t('editor.basic.totpCopied'))
  } catch (e) {
    notify.error(e)
  }
}
</script>

<template>
  <div class="totp" role="group" :aria-label="t('editor.basic.totpCode')">
    <template v-if="code">
      <span class="code mono">{{ grouped }}</span>
      <span class="hint">{{ t('editor.basic.totpLeft', { n: left }) }}</span>
      <UiButton size="sm" icon="copy" @click="copy">{{ t('editor.basic.totpCopy') }}</UiButton>
    </template>
    <span v-else-if="error" class="error-text">{{ error }}</span>
  </div>
</template>

<style scoped>
.totp {
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 32px;
}

.code {
  font-size: 20px;
  font-weight: 600;
  letter-spacing: 1px;
}
</style>
