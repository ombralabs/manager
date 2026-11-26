<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FieldError, ProxyRecord } from '../api/types'
import { useFieldErrors } from '../lib/errorText'
import { kindOptions } from '../lib/proxyOptions'
import { useClearFieldErrors } from '../lib/useClearFieldErrors'
import { fieldsOrToast, notify } from '../lib/notify'
import { type ProxyEditDraft, draftFromRow, emptyProxyDraft, proxyDraftErrors, toProxyEditForm } from '../lib/proxies'
import { useProxiesStore } from '../stores/proxies'
import SecretField from './SecretField.vue'
import UiButton from './UiButton.vue'
import UiDialog from './UiDialog.vue'
import UiField from './UiField.vue'
import UiSegmented from './UiSegmented.vue'
import UiSwitch from './UiSwitch.vue'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ proxy: ProxyRecord | null }>()
const emit = defineEmits<{ saved: [record: ProxyRecord] }>()
const { t } = useI18n()
const proxies = useProxiesStore()

const draft = ref<ProxyEditDraft>(emptyProxyDraft())
const errors = ref<FieldError[]>([])
const saving = ref(false)
const err = useFieldErrors(() => errors.value)

/** Открытие — черновик из записи или пустой; закрытие стирает черновик, чтобы пароль не оставался в памяти. */
watch(
  open,
  (isOpen) => {
    draft.value = isOpen && props.proxy ? draftFromRow(props.proxy) : emptyProxyDraft()
    errors.value = []
  },
  { immediate: true },
)

/** Поле ошибки → значение в черновике. */
const DRAFT_FIELDS: Record<string, (value: ProxyEditDraft) => unknown> = {
  proxy_kind: (value) => value.kind,
  proxy_host: (value) => value.host,
  proxy_port: (value) => value.port,
  proxy_username: (value) => value.username,
  proxy_password: (value) => value.password,
  note: (value) => value.note,
}
useClearFieldErrors(errors, draft, (field) => JSON.stringify(DRAFT_FIELDS[field]?.(draft.value)))

/** Ошибки полей — у полей, диалог остаётся открытым; прочие ошибки — уведомлением. */
async function submit() {
  const local = proxyDraftErrors(draft.value)
  errors.value = local
  if (local.length > 0) return
  saving.value = true
  try {
    const form = toProxyEditForm(draft.value)
    const record = props.proxy ? await proxies.update(props.proxy.id, form) : await proxies.create(form)
    notify.info(t(props.proxy ? 'proxyDialog.saved' : 'proxyDialog.added'))
    emit('saved', record)
    open.value = false
  } catch (e) {
    fieldsOrToast(e, errors)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <UiDialog
    v-model:open="open"
    :title="proxy ? t('proxyDialog.editTitle') : t('proxyDialog.addTitle')"
    :width="560"
    :busy="saving"
  >
    <UiField :label="t('proxyDialog.kind')">
      <div class="kind">
        <UiSegmented v-model="draft.kind" :options="kindOptions" :aria-label="t('proxyDialog.kind')" />
      </div>
    </UiField>
    <div class="address">
      <UiField :label="t('proxyDialog.host')" input-id="proxy-dialog-host" :hint="t('proxyDialog.hostHint')" :error="err('proxy_host')">
        <input
          id="proxy-dialog-host"
          v-model="draft.host"
          class="input mono"
          :class="{ invalid: err('proxy_host') }"
          autocomplete="off"
          spellcheck="false"
          placeholder="gw.example.net"
        />
      </UiField>
      <UiField :label="t('proxyDialog.port')" input-id="proxy-dialog-port" :error="err('proxy_port')">
        <input
          id="proxy-dialog-port"
          v-model="draft.port"
          class="input mono"
          :class="{ invalid: err('proxy_port') }"
          inputmode="numeric"
          placeholder="1080"
        />
      </UiField>
    </div>
    <div class="grid two">
      <UiField :label="t('proxyDialog.username')" input-id="proxy-dialog-username" :error="err('proxy_username')">
        <input
          id="proxy-dialog-username"
          v-model="draft.username"
          class="input"
          :class="{ invalid: err('proxy_username') }"
          autocomplete="off"
          spellcheck="false"
        />
      </UiField>
      <UiField :label="t('proxyDialog.password')" input-id="proxy-dialog-password" :error="err('proxy_password')">
        <SecretField v-model="draft.password" input-id="proxy-dialog-password" :invalid="err('proxy_password') !== null" />
      </UiField>
    </div>
    <UiField :label="t('proxyDialog.note')" input-id="proxy-dialog-note" :error="err('note')">
      <input id="proxy-dialog-note" v-model="draft.note" class="input" :class="{ invalid: err('note') }" />
    </UiField>
    <div class="switch-row">
      <div class="switch-text">
        <span>{{ t('proxyDialog.enabled') }}</span>
        <span class="hint">{{ t('proxyDialog.enabledHint') }}</span>
      </div>
      <UiSwitch v-model="draft.enabled" :label="t('proxyDialog.enabled')" />
    </div>
    <template #footer>
      <UiButton :disabled="saving" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" :loading="saving" @click="submit">
        {{ proxy ? t('common.save') : t('proxyDialog.submitAdd') }}
      </UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.kind {
  width: 360px;
}

.address {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 140px;
  gap: 16px;
}

.switch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.switch-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
</style>
