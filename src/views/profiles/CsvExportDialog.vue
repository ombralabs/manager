<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { pickSaveCsv } from '../../api/dialogs'
import UiButton from '../../components/UiButton.vue'
import UiCheckbox from '../../components/UiCheckbox.vue'
import UiDialog from '../../components/UiDialog.vue'
import { localDate } from '../../lib/format'
import { notify } from '../../lib/notify'
import { useProfilesStore } from '../../stores/profiles'

const open = defineModel<boolean>('open', { required: true })
const { t } = useI18n()
const profiles = useProfilesStore()

const withSecrets = ref(false)
const busy = ref(false)

// Флажок секретов не переживает закрытие диалога.
watch(open, (value) => {
  if (!value) withSecrets.value = false
})

/** Отмена окна сохранения оставляет диалог открытым. */
async function save() {
  busy.value = true
  try {
    const path = await pickSaveCsv(`ombra-profiles-${localDate(new Date())}.csv`)
    if (path === null) return
    const count = await core.exportCsvFile(profiles.listQuery(), withSecrets.value, path)
    open.value = false
    notify.info(t('profiles.csvExport.done', count))
  } catch (e) {
    notify.error(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('profiles.csvExport.title')" :width="480" :busy="busy">
    <p class="hint">{{ t('profiles.csvExport.text', profiles.rows.length) }}</p>
    <label class="secrets">
      <UiCheckbox v-model="withSecrets" :ariaLabel="t('profiles.csvExport.secrets')" />
      <span>{{ t('profiles.csvExport.secrets') }}</span>
    </label>
    <div v-if="withSecrets" class="notice">{{ t('profiles.csvExport.secretsWarning') }}</div>
    <template #footer>
      <UiButton :disabled="busy" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" icon="upload" :loading="busy" :disabled="profiles.rows.length === 0" @click="save">
        {{ t('profiles.csvExport.submit') }}
      </UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.secrets {
  display: flex;
  align-items: center;
  gap: 8px;
}
</style>
