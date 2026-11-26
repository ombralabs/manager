<script setup lang="ts">
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import QRCode from 'qrcode'
import { computed, nextTick, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { SecuritySetup } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiCheckbox from '../../components/UiCheckbox.vue'
import UiField from '../../components/UiField.vue'
import { notify } from '../../lib/notify'
import { useSecurityStore } from '../../stores/security'

const { t } = useI18n()
const security = useSecurityStore()
const stage = ref<0 | 1 | 2 | 3>(0)
const operation = ref<'setup' | 'password' | 'authenticator'>('setup')
const password = ref('')
const confirmation = ref('')
const current = ref('')
const code = ref('')
const setup = ref<SecuritySetup | null>(null)
const qr = ref('')
const saved = ref(false)
const localError = ref('')
const heading = ref<HTMLElement | null>(null)
const validPassword = computed(() => [...password.value].length >= 12 && password.value === confirmation.value)
const error = computed(() => localError.value || security.error)
async function focusHeading() { await nextTick(); heading.value?.focus({ preventScroll: true }) }
function clearSecrets() { password.value = confirmation.value = current.value = code.value = qr.value = ''; setup.value = null; saved.value = false }
function start(kind: typeof operation.value) { clearSecrets(); operation.value = kind; stage.value = 1; localError.value = security.error = ''; void focusHeading() }
async function cancel() {
  if (security.busy) return
  await core.securityCancelSetup().catch(() => {})
  clearSecrets(); stage.value = 0; security.error = localError.value = ''
}
onBeforeUnmount(() => { if (stage.value && !security.busy) void core.securityCancelSetup().catch(() => {}); clearSecrets() })
async function begin() {
  if (security.busy) return
  localError.value = ''
  if (operation.value !== 'authenticator' && !validPassword.value) { localError.value = t('security.passwordHint'); return }
  try {
    await security.run(async () => {
      if (operation.value === 'password') {
        const recovery_codes = await core.securityChangePassword(current.value, code.value.trim(), password.value)
        setup.value = { secret: '', uri: '', recovery_codes }; stage.value = 3
      } else {
        setup.value = operation.value === 'setup' ? await core.securityBeginSetup(password.value)
          : await core.securityReplaceAuthenticator(current.value, code.value.trim())
        qr.value = await QRCode.toDataURL(setup.value.uri, { width: 176, margin: 4, errorCorrectionLevel: 'M' })
        stage.value = 2
      }
      password.value = confirmation.value = current.value = code.value = ''
    })
    await focusHeading()
  } catch { /* Ошибка показана рядом с формой. */ }
}
async function verify() {
  if (security.busy) return
  try { await security.run(() => core.securityVerifySetup(code.value.trim())); code.value = ''; stage.value = 3; await focusHeading() }
  catch { code.value = '' }
}
async function finish() {
  if (security.busy || !saved.value) return
  try {
    if (operation.value !== 'password') await security.run(() => core.securityFinishSetup(saved.value))
    clearSecrets(); stage.value = 0
  } catch {
    await core.securityCancelSetup().catch(() => {})
    clearSecrets(); stage.value = 0
  }
}
async function copy(value: string) { try { await writeText(value) } catch (e) { notify.error(e) } }
</script>

<template>
  <section id="local-security" class="card section security-card" data-test="security-card" :aria-busy="security.busy">
    <header><h2 ref="heading" tabindex="-1" class="section-title">{{ t('security.title') }}</h2><span v-if="security.status?.configured" class="security-state">{{ t(security.status.migrating ? 'security.migrationPending' : 'security.enabled') }}</span></header>
    <p v-if="security.status?.supported === false" class="hint" role="status">{{ t('security.unsupported') }}</p>
    <template v-else-if="!stage">
      <p class="hint">{{ t(security.status?.migrating ? 'security.migrationPendingHint' : security.status?.configured ? 'security.protectedHint' : 'security.setupHint') }}</p>
      <p v-if="security.status?.configured" class="hint">{{ t('security.remaining', { n: security.status.recovery_remaining }) }}</p>
      <p v-if="security.status?.recovery_recent" class="hint" role="status">{{ t('security.recoveredHint') }}</p>
      <div class="actions">
        <UiButton v-if="!security.status?.configured" variant="primary" @click="start('setup')">{{ t('security.configure') }}</UiButton>
        <template v-else-if="security.unlocked">
          <UiButton @click="start('password')">{{ t('security.changePassword') }}</UiButton>
          <UiButton @click="start('authenticator')">{{ t('security.replaceAuthenticator') }}</UiButton>
        </template>
        <RouterLink v-else class="link" to="/profiles">{{ t('security.unlock') }}</RouterLink>
      </div>
    </template>
    <template v-else>
      <ol class="security-steps" :aria-label="t('security.setupSteps')">
        <li v-for="(key, index) in ['passwordStep', 'authenticatorStep', 'recoveryStep']" :key="key" :aria-current="stage === index + 1 ? 'step' : undefined"><span>{{ index + 1 }}</span>{{ t('security.' + key) }}</li>
      </ol>
      <form v-if="stage === 1" class="security-form" @submit.prevent="begin">
        <template v-if="operation !== 'setup'">
          <UiField :label="t('security.currentPassword')" input-id="security-current"><input id="security-current" v-model="current" class="input" type="password" autocomplete="current-password" required :disabled="security.busy" /></UiField>
          <UiField v-if="operation !== 'authenticator' || !security.status?.recovery_recent" :label="t('security.reauthCode')" input-id="security-current-code"><input id="security-current-code" v-model="code" class="input mono" autocomplete="one-time-code" required :disabled="security.busy" /></UiField>
        </template>
        <template v-if="operation !== 'authenticator'">
          <UiField :label="t('security.newPassword')" input-id="security-password" :hint="t('security.passwordHint')"><input id="security-password" v-model="password" class="input" type="password" autocomplete="new-password" required :disabled="security.busy" /></UiField>
          <UiField :label="t('security.confirmPassword')" input-id="security-confirm"><input id="security-confirm" v-model="confirmation" class="input" type="password" autocomplete="new-password" required :disabled="security.busy" /></UiField>
        </template>
        <div class="actions"><UiButton variant="primary" type="submit" :loading="security.busy" :disabled="operation !== 'authenticator' && !validPassword">{{ t('common.next') }}</UiButton><UiButton :disabled="security.busy" @click="cancel">{{ t('common.cancel') }}</UiButton></div>
      </form>
      <form v-else-if="stage === 2 && setup" class="security-form" @submit.prevent="verify">
        <p class="hint">{{ t('security.scanHint') }}</p>
        <div class="authenticator-secret"><img :src="qr" width="176" height="176" :alt="t('security.qrAlt')" /><div><span class="label">{{ t('security.manualKey') }}</span><code class="setup-secret">{{ setup.secret }}</code><UiButton size="sm" icon="copy" @click="copy(setup!.secret)">{{ t('common.copy') }}</UiButton></div></div>
        <UiField :label="t('security.code')" input-id="security-verify-code"><input id="security-verify-code" v-model="code" class="input mono" inputmode="numeric" autocomplete="one-time-code" required :disabled="security.busy" /></UiField>
        <div class="actions"><UiButton variant="primary" type="submit" :loading="security.busy" :disabled="!/^\d{6}$/.test(code)">{{ t('security.verify') }}</UiButton><UiButton :disabled="security.busy" @click="cancel">{{ t('common.cancel') }}</UiButton></div>
      </form>
      <div v-else-if="stage === 3 && setup" class="security-form">
        <p class="hint">{{ t('security.recoveryHint') }}</p>
        <div class="recovery-codes"><code v-for="value in setup.recovery_codes" :key="value">{{ value }}</code></div>
        <UiButton icon="copy" @click="copy(setup!.recovery_codes.join('\n'))">{{ t('security.copyRecovery') }}</UiButton>
        <label class="saved-recovery"><UiCheckbox v-model="saved" :ariaLabel="t('security.recoverySaved')" :disabled="security.busy" /><span>{{ t('security.recoverySaved') }}</span></label>
        <p v-if="operation === 'setup'" class="hint">{{ t('security.migrationHint') }}</p>
        <p v-if="security.busy" class="hint" role="status">{{ t('security.protecting') }}</p>
        <UiButton variant="primary" :loading="security.busy" :disabled="!saved" @click="finish">{{ t(operation === 'setup' ? 'security.enable' : 'common.done') }}</UiButton>
        <UiButton v-if="operation !== 'password'" :disabled="security.busy" @click="cancel">{{ t('common.cancel') }}</UiButton>
      </div>
    </template>
    <p v-if="error" class="error-text" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
.security-card { display: flex; flex-direction: column; gap: 16px; padding: 20px; }
.security-card > header { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
.security-card h2:focus { outline: none; }
.security-card p { margin: 0; }
.security-state { color: var(--accent-ink); font-size: 12px; }
.actions { display: flex; flex-wrap: wrap; gap: 10px; }
.security-steps { display: flex; flex-wrap: wrap; gap: 14px; margin: 0; padding: 0 0 16px; list-style: none; border-bottom: 1px solid var(--border); }
.security-steps li { display: flex; align-items: center; gap: 7px; color: var(--text-2); font-size: 12px; }
.security-steps li span { display: grid; place-items: center; width: 24px; height: 24px; border: 1px solid var(--border); border-radius: 50%; }
.security-steps [aria-current] { color: var(--accent-ink); font-weight: 500; }
.security-steps [aria-current] span { border-color: transparent; background: var(--accent-soft); }
.security-form { display: flex; flex-direction: column; gap: 16px; max-width: 640px; }
.security-form > .btn { align-self: flex-start; }
.authenticator-secret { display: flex; flex-wrap: wrap; align-items: center; gap: 24px; }
.authenticator-secret > div { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; min-width: 0; flex: 1 1 180px; }
.authenticator-secret img { border: 1px solid var(--border); border-radius: var(--radius-control); background: #fff; padding: 8px; }
.setup-secret { word-break: break-all; font-size: 13px; }
.recovery-codes { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 12px; padding: 16px; border: 1px solid var(--border); border-radius: var(--radius-control); background: var(--muted-bg); }
.recovery-codes code { font-size: 12px; overflow-wrap: anywhere; }
.saved-recovery { display: flex; align-items: flex-start; gap: 10px; font-size: 13px; cursor: pointer; }
.saved-recovery .checkbox { flex-shrink: 0; }
</style>
