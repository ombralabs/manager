<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { toCoreError } from '../../api/errors'
import type { ProfileRecord } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import { notify } from '../../lib/notify'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ path: string }>()
const emit = defineEmits<{ imported: [record: ProfileRecord] }>()
const { t } = useI18n()

const password = ref('')
const wrong = ref(false)
const busy = ref(false)

// Пароль живёт только пока открыт диалог.
watch(open, (value) => {
  if (!value) {
    password.value = ''
    wrong.value = false
  }
})

async function submit() {
  if (busy.value || password.value === '') return
  busy.value = true
  try {
    const record = await core.importProfile(props.path, password.value)
    open.value = false
    emit('imported', record)
  } catch (e) {
    const err = toCoreError(e)
    if (err.code === 'ArchivePassword') {
      wrong.value = true
    } else {
      open.value = false
      notify.error(err)
    }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <UiDialog
    v-model:open="open"
    :title="t('profiles.importMenu.passwordTitle')"
    :description="t('profiles.importMenu.passwordText')"
    :width="480"
    :busy="busy"
  >
    <UiField :label="t('profiles.importMenu.password')" input-id="archive-password" :error="wrong ? t('profiles.importMenu.passwordWrong') : null">
      <input
        id="archive-password"
        v-model="password"
        class="input"
        type="password"
        autocomplete="off"
        :readonly="busy"
        @input="wrong = false"
        @keydown.enter="submit"
      />
    </UiField>
    <template #footer>
      <UiButton :disabled="busy" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" :loading="busy" :disabled="password === ''" @click="submit">
        {{ t('profiles.importMenu.submit') }}
      </UiButton>
    </template>
  </UiDialog>
</template>
