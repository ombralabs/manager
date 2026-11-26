<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { GroupRecord } from '../../api/types'
import ExtRiskNote from '../../components/ExtRiskNote.vue'
import type { SelectOption } from '../../components/types'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import UiSelect from '../../components/UiSelect.vue'
import { showBulkReport } from '../../lib/bulkReport'
import { setLabel } from '../../lib/extensions'
import { notify } from '../../lib/notify'
import { useExtensionsStore } from '../../stores/extensions'
import { useGroupsStore } from '../../stores/groups'

const open = defineModel<boolean>('open', { required: true })
/** `group` — `null` для «Без группы»; `count` — живые профили группы. Набор получают только они (как в AdsPower). */
const props = defineProps<{ group: GroupRecord | null; count: number }>()
const { t } = useI18n()
const store = useExtensionsStore()
const groups = useGroupsStore()

/** Значение `UiSelect` — строкой; по умолчанию «Без набора». */
const setId = ref('0')
const busy = ref(false)

watch(
  open,
  (value) => {
    if (!value) return
    setId.value = '0'
    store.refresh().catch(notify.error)
  },
  { immediate: true },
)

/** Пока список грузится — один пункт «…», а не номер набора в поле. */
const options = computed<SelectOption[]>(() =>
  store.loaded ? store.categories.map((set) => ({ value: String(set.id), label: setLabel(set) })) : [{ value: setId.value, label: '…' }],
)
const risky = computed(() => store.risky(store.categoryById(Number(setId.value))))
const title = computed(() => t('groups.extSetTitle', { name: props.group?.name ?? t('common.ungrouped') }))

async function submit() {
  if (busy.value) return
  busy.value = true
  try {
    showBulkReport('extSet', await core.setGroupExtCategory(props.group?.id ?? null, Number(setId.value)))
    open.value = false
    // Строки профилей несут набор — панель группы покажет новый.
    await Promise.all([store.refresh().catch(notify.error), groups.refresh().catch(notify.error)])
  } catch (e) {
    notify.error(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="title" :width="480" :busy="busy">
    <UiField :label="t('groups.extSetLabel')" input-id="group-ext-set">
      <UiSelect id="group-ext-set" v-model="setId" :options="options" :disabled="busy || !store.loaded" />
    </UiField>
    <ExtRiskNote :extensions="risky" />
    <p class="hint text">{{ t('groups.extSetText', { n: count }) }}</p>
    <template #footer>
      <UiButton :disabled="busy" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" :loading="busy" :disabled="!store.loaded" @click="submit">{{ t('groups.extSetSubmit') }}</UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.text {
  margin: 0;
}
</style>
