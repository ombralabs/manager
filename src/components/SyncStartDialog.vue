<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProfileRow } from '../api/types'
import { notify } from '../lib/notify'
import { useSyncStore } from '../stores/sync'
import UiButton from './UiButton.vue'
import UiDialog from './UiDialog.vue'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ profiles: Pick<ProfileRow, 'id' | 'name'>[] }>()
const emit = defineEmits<{ started: [] }>()
const { t } = useI18n()
const sync = useSyncStore()
const main = ref('')
const busy = ref(false)
watch(open, (value) => { if (value) main.value = '' })

async function start() {
  if (busy.value || !main.value || props.profiles.length < 2) return
  busy.value = true
  try {
    sync.pick(props.profiles.map((p) => p.id))
    sync.makeMain(main.value)
    await sync.start()
    open.value = false
    emit('started')
  } catch (e) {
    notify.error(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('sync.launch.title')" :description="t('sync.launch.description')" :busy="busy">
    <p class="warning">{{ t('sync.launch.warning') }}</p>
    <fieldset :disabled="busy" class="choices">
      <legend class="hint">{{ t('sync.launch.choose') }}</legend>
      <label v-for="profile in profiles" :key="profile.id" class="choice" :class="{ chosen: main === profile.id }">
        <input v-model="main" type="radio" name="sync-master" :value="profile.id" />
        <span :title="profile.name">{{ profile.name }}</span>
      </label>
    </fieldset>
    <template #footer>
      <UiButton :busy="busy" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton class="sync-confirm" variant="primary" icon="sync" :disabled="!main || profiles.length < 2" :loading="busy" @click="start">
        {{ t('sync.launch.confirm') }}
      </UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.warning { margin: 0; color: var(--text-2); line-height: 1.5; }
.choices { display: flex; flex-direction: column; gap: 6px; margin: 0; padding: 0; border: 0; min-width: 0; }
legend { margin-bottom: 8px; }
.choice { display: flex; align-items: center; gap: 10px; padding: 10px 12px; border: 1px solid var(--border); border-radius: 8px; cursor: pointer; }
.choice.chosen { border-color: var(--accent); background: var(--accent-soft); }
.choice span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.choice input { margin: 0; flex-shrink: 0; accent-color: var(--accent); }
.choice:focus-within { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
