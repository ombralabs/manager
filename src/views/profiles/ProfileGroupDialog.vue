<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { toCoreError } from '../../api/errors'
import type { GroupRecord } from '../../api/types'
import ColorPalette from '../../components/ColorPalette.vue'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import { nextGroupColor } from '../../lib/groups'
import { notify } from '../../lib/notify'
import { useGroupsStore } from '../../stores/groups'

const open = defineModel<boolean>('open', { required: true })
const emit = defineEmits<{ created: [group: GroupRecord] }>()
const { t } = useI18n()
const groups = useGroupsStore()
const name = ref('')
const color = ref(nextGroupColor(groups.groups))
const error = ref('')
const saving = ref(false)
watch(name, () => { error.value = '' })

async function save() {
  if (!name.value.trim() || [...name.value.trim()].length > 50) { error.value = t('profiles.groupSidebar.nameRequired'); return }
  saving.value = true
  try {
    const created = await groups.create({ name: name.value.trim(), color: color.value, note: '', quick_access: false })
    emit('created', created)
    open.value = false
  } catch (e) {
    const err = toCoreError(e)
    if (err.code === 'GroupExists') error.value = t('errors.GroupExists')
    else if (err.code === 'Validation') error.value = t('profiles.groupSidebar.nameRequired')
    else notify.error(e)
  } finally { saving.value = false }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('groups.new')" :width="360" :busy="saving">
    <form id="profile-group-form" class="group-form" @submit.prevent="save">
      <UiField :label="t('groups.columns.name')" input-id="profile-group-name" :error="error">
        <input id="profile-group-name" v-model="name" class="input" maxlength="50" :disabled="saving" :aria-invalid="!!error" />
      </UiField>
      <UiField :label="t('groups.columns.color')"><ColorPalette v-model="color" /></UiField>
    </form>
    <template #footer>
      <UiButton :disabled="saving" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" type="submit" form="profile-group-form" :loading="saving">{{ t('common.save') }}</UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.group-form { display: flex; flex-direction: column; gap: 20px; }
</style>
