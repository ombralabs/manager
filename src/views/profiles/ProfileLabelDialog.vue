<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProfileLabel } from '../../api/types'
import { toCoreError } from '../../api/errors'
import ColorPalette from '../../components/ColorPalette.vue'
import ConfirmDialog from '../../components/ConfirmDialog.vue'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import { notify } from '../../lib/notify'
import { labelStyle } from '../../lib/profileLabels'
import { useProfileLabelsStore } from '../../stores/profileLabels'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ label: ProfileLabel | null }>()
const emit = defineEmits<{ saved: [label: ProfileLabel] }>()
const { t } = useI18n()
const labels = useProfileLabelsStore()
const name = ref(props.label?.name ?? '')
const color = ref(props.label?.color ?? '#2447c9')
const error = ref('')
const busy = ref(false)
const deleting = ref(false)
const preview = computed(() => labelStyle({ color: color.value }))

async function save() {
  const title = name.value.trim()
  if (!title || [...title].length > 32 || /[\u0000-\u001f\u007f]/.test(title)) { error.value = t('profiles.labels.invalidName'); return }
  busy.value = true
  error.value = ''
  try {
    const saved = await labels.save(props.label?.id ?? null, title, color.value)
    emit('saved', saved)
    open.value = false
  } catch (e) {
    const err = toCoreError(e)
    if (err.code === 'Validation') error.value = t(err.fields.some((field) => field.code === 'taken') ? 'profiles.labels.nameTaken' : 'profiles.labels.invalidName')
    else notify.error(e)
  } finally { busy.value = false }
}

async function remove() {
  if (!props.label) return
  busy.value = true
  try {
    await labels.remove(props.label.id)
    deleting.value = false
    open.value = false
  } catch (e) { notify.error(e) }
  finally { busy.value = false }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t(label ? 'profiles.labels.edit' : 'profiles.labels.create')" :width="400" :busy="busy">
    <form id="profile-label-form" class="label-form" @submit.prevent="save">
      <UiField :label="t('profiles.labels.name')" input-id="profile-label-name" :error="error">
        <input id="profile-label-name" v-model="name" class="input" maxlength="32" :disabled="busy" :aria-invalid="!!error" @input="error = ''" />
      </UiField>
      <UiField :label="t('profiles.labels.color')">
        <div class="label-colors"><ColorPalette v-model="color" /><input v-model="color" type="color" :disabled="busy" :aria-label="t('profiles.labels.customColor')" /></div>
      </UiField>
      <div class="label-preview"><span class="profile-label-chip" :style="preview">{{ name.trim() || t('profiles.labels.preview') }}</span></div>
    </form>
    <template #footer>
      <UiButton v-if="label" variant="ghost" class="delete-label" :disabled="busy" @click="deleting = true">{{ t('common.delete') }}</UiButton>
      <UiButton :disabled="busy" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" form="profile-label-form" type="submit" :loading="busy">{{ t('common.save') }}</UiButton>
    </template>
  </UiDialog>
  <ConfirmDialog v-if="label" v-model:open="deleting" :title="t('profiles.labels.deleteTitle', { name: label.name })" :description="t('profiles.labels.deleteText')" :confirm-label="t('common.delete')" :busy="busy" danger @confirm="remove" />
</template>

<style scoped>
.label-form { display: flex; flex-direction: column; gap: 20px; }
.label-colors { display: flex; align-items: flex-start; gap: 24px; }
.label-colors input { width: 36px; height: 36px; padding: 2px; border: 1px solid var(--border-strong); border-radius: 6px; background: var(--surface); cursor: pointer; }
.label-preview { display: flex; align-items: center; min-height: 48px; padding: 12px; border-radius: 8px; background: var(--muted-bg); }
.delete-label { margin-right: auto; color: var(--danger-text); }
</style>
