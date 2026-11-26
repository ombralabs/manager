<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { pickSaveArchive } from '../../api/dialogs'
import type { ProfileRow } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import { archiveFileName } from '../../lib/cookieExport'
import { notify } from '../../lib/notify'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ row: ProfileRow | null }>()
const { t } = useI18n()

const password = ref('')
const confirm = ref('')
const busy = ref(false)
const mismatch = computed(() => confirm.value !== '' && password.value !== confirm.value)
const ready = computed(() => props.row !== null && password.value === confirm.value)

// Пароль живёт только пока открыт диалог.
watch(open, (value) => {
  if (!value) password.value = confirm.value = ''
})

/** Отмена окна сохранения оставляет диалог открытым; без пароля секреты в архив не попадают. */
async function save() {
  const row = props.row
  if (!row) return
  busy.value = true
  try {
    const path = await pickSaveArchive(archiveFileName(row.name))
    if (path === null) return
    await core.exportProfile(row.id, path, password.value === '' ? null : password.value)
    open.value = false
    notify.info(t('profiles.archiveExport.done'))
  } catch (e) {
    notify.error(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('profiles.archiveExport.title')" :width="480" :busy="busy">
    <UiField :label="t('profiles.archiveExport.password')" input-id="archive-export-password">
      <input id="archive-export-password" v-model="password" class="input" type="password" autocomplete="new-password" />
    </UiField>
    <UiField
      :label="t('profiles.archiveExport.confirm')"
      input-id="archive-export-confirm"
      :error="mismatch ? t('profiles.archiveExport.mismatch') : null"
    >
      <input id="archive-export-confirm" v-model="confirm" class="input" type="password" autocomplete="new-password" />
    </UiField>
    <p class="hint">{{ t('profiles.archiveExport.hint') }}</p>
    <template #footer>
      <UiButton :disabled="busy" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" icon="upload" :loading="busy" :disabled="!ready" @click="save">
        {{ t('profiles.archiveExport.submit') }}
      </UiButton>
    </template>
  </UiDialog>
</template>
