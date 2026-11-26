<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProfileLabelFilter } from '../api/types'
import { notify } from '../lib/notify'
import { useProfileLabelsStore } from '../stores/profileLabels'
import UiSelect from './UiSelect.vue'

const model = defineModel<ProfileLabelFilter>({ required: true })
const props = withDefaults(defineProps<{ id?: string; active?: boolean }>(), { active: true })
const { t } = useI18n()
const labels = useProfileLabelsStore()
const failed = ref(false)
const options = computed(() => [
  { value: 'any', label: t('profiles.statusFilter.any') },
  { value: 'none', label: t('profiles.labels.none') },
  ...labels.labels.map((label) => ({ value: String(label.id), label: label.name })),
])
const value = computed({
  get: () => String(model.value),
  set: (value: string) => { model.value = value === 'any' || value === 'none' ? value : Number(value) },
})

async function load() {
  failed.value = false
  try { await labels.refresh() }
  catch (e) { failed.value = true; notify.error(e) }
}
watch(() => props.active, (active) => { if (active) void load() }, { immediate: true })
watch([() => labels.loaded, () => labels.labels, model], () => {
  if (labels.loaded && typeof model.value === 'number' && !labels.labels.some((label) => label.id === model.value)) model.value = 'any'
})
</script>

<template>
  <div class="profile-status-filter">
    <UiSelect :id="id" v-model="value" :options="options" :disabled="!labels.loaded" :aria-label="t('profiles.statusFilter.label')" />
    <button v-if="failed" type="button" class="link" @click="load">{{ t('common.retry') }}</button>
  </div>
</template>

<style scoped>
.profile-status-filter { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
</style>
