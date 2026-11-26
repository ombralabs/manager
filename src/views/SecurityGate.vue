<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { core } from '../api/core'
import { useSecurityStore } from '../stores/security'
import AppBanners from '../components/AppBanners.vue'
import AppIcon from '../components/AppIcon.vue'
import UiButton from '../components/UiButton.vue'
import UiField from '../components/UiField.vue'

const { t } = useI18n()
const router = useRouter()
const security = useSecurityStore()
const password = ref('')
const code = ref('')
const recovery = ref(false)
const retry = ref(security.status?.retry_after ?? 0)
const timer = setInterval(() => { retry.value = Math.max(0, retry.value - 1) }, 1000)
onBeforeUnmount(() => { clearInterval(timer); password.value = code.value = '' })
async function unlock() {
  if (security.busy || retry.value > 0) return
  try {
    await security.run(() => core.securityUnlock(password.value, code.value.trim(), recovery.value))
    password.value = code.value = ''
    if (recovery.value) await router.push('/settings#security')
  } catch { retry.value = security.status?.retry_after ?? 0; code.value = '' }
}
</script>

<template>
  <main class="page security-gate">
    <AppBanners security-notice />
    <section class="unlock-card">
      <AppIcon name="shield" :size="34" />
      <h1>{{ t(security.status?.configured ? 'security.lockedTitle' : 'security.setupTitle') }}</h1>
      <p class="hint">{{ t(security.status?.configured ? 'security.unlockHint' : 'security.setupHint') }}</p>
      <form v-if="security.status?.configured" class="unlock-form" @submit.prevent="unlock">
        <UiField :label="t('security.password')" input-id="unlock-password"><input id="unlock-password" v-model="password" class="input" type="password" autocomplete="current-password" required :disabled="security.busy" /></UiField>
        <UiField :label="t(recovery ? 'security.recoveryCode' : 'security.code')" input-id="unlock-code"><input id="unlock-code" v-model="code" class="input mono" :inputmode="recovery ? 'text' : 'numeric'" :autocomplete="recovery ? 'off' : 'one-time-code'" required :disabled="security.busy" /></UiField>
        <p v-if="security.error" class="error-text" role="alert">{{ security.error }}</p>
        <p v-if="retry" class="hint" role="status">{{ t('security.retry', { n: retry }) }}</p>
        <UiButton type="submit" variant="primary" :loading="security.busy" :disabled="!password || !code || retry > 0">{{ t('security.unlock') }}</UiButton>
        <button type="button" class="link" :disabled="security.busy" @click="recovery = !recovery; code = ''; security.error = ''">{{ t(recovery ? 'security.useAuthenticator' : 'security.useRecovery') }}</button>
      </form>
      <UiButton v-else variant="primary" @click="router.push('/settings#security')">{{ t('security.configure') }}</UiButton>
    </section>
  </main>
</template>

<style scoped>
.security-gate { gap: 32px; }
.unlock-card { width: 100%; max-width: 440px; margin: auto; padding: 24px 12px; text-align: center; }
.unlock-card > .icon { color: var(--accent-ink); }
.unlock-card h1 { margin: 18px 0 10px; font-size: 24px; line-height: 32px; }
.unlock-card > p { margin: 0 0 24px; }
.unlock-form { display: flex; flex-direction: column; gap: 16px; text-align: left; }
.unlock-form > .link { align-self: center; font-size: 13px; }
.unlock-form > p { margin: 0; }
</style>
