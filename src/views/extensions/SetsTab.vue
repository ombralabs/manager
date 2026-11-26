<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { ExtCategory } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import ConfirmDialog from '../../components/ConfirmDialog.vue'
import UiButton from '../../components/UiButton.vue'
import { toggledItems } from '../../lib/extensions'
import { notify } from '../../lib/notify'
import { useConfirm } from '../../lib/useConfirm'
import { useExtensionsStore } from '../../stores/extensions'
import ExtCategoryDialog from './ExtCategoryDialog.vue'
import SetComposition from './SetComposition.vue'
import SetsList from './SetsList.vue'

const { t } = useI18n()
const store = useExtensionsStore()

/** Наборы кроме 0: его роль — «включено» на вкладке «Все расширения». */
const sets = computed(() => store.categories.filter((set) => set.id !== 0))
/** Выбранный набор; не выбран, удалён или пропал после перечитывания — первый. */
const selectedId = ref<number | null>(null)
const selected = computed(() => sets.value.find((set) => set.id === selectedId.value) ?? sets.value[0] ?? null)

/** Состав сохраняется: переключатели `aria-disabled`, второй щелчок не уйдёт со старым составом. */
const saving = ref(false)
async function toggle(id: string, on: boolean) {
  const set = selected.value
  if (!set || saving.value) return
  saving.value = true
  try {
    await store.setItems(set.id, toggledItems(store.extensions, set.extension_ids, id, on))
  } catch (e) {
    notify.error(e)
  } finally {
    saving.value = false
  }
}

const dialogOpen = ref(false)
/** Набор в диалоге; `null` — новый. */
const editing = ref<ExtCategory | null>(null)
function openDialog(set: ExtCategory | null) {
  editing.value = set
  dialogOpen.value = true
}
/** Новый набор становится выбранным; переименованный остаётся, каким был. */
async function onSaved(set: ExtCategory) {
  await store.refresh().catch(notify.error)
  if (editing.value === null) selectedId.value = set.id
}

const removal = useConfirm(async (set: ExtCategory) => {
  await core.deleteExtCategory(set.id)
  notify.info(t('extensions.setDeleted', { name: set.name }))
  if (selectedId.value === set.id) selectedId.value = null
  await store.refresh().catch(notify.error)
})

/** «Новый набор» в шапке экрана. */
defineExpose({ create: () => openDialog(null) })
</script>

<template>
  <p class="note"><AppIcon name="info" />{{ t('extensions.infoSets') }}</p>
  <div class="layout">
    <SetsList
      :sets="sets"
      :selected="selected?.id ?? null"
      :library="store.extensions"
      :risky="store.riskyIds"
      @select="selectedId = $event"
      @rename="openDialog"
      @remove="removal.ask"
    />
    <SetComposition
      v-if="selected && store.extensions.length > 0"
      :set="selected"
      :library="store.extensions"
      :enabled="store.enabledIds"
      :pending="store.pending"
      :saving="saving"
      @toggle="toggle"
    />
    <section v-else-if="store.loaded" class="placeholder card">
      <template v-if="!selected">
        <div class="hint">{{ t('extensions.noSetsText') }}</div>
        <UiButton variant="primary" icon="plus" @click="openDialog(null)">{{ t('extensions.newSet') }}</UiButton>
      </template>
      <template v-else>
        <div class="hint">{{ t('extensions.emptyLibrary') }}</div>
        <RouterLink to="/extensions" class="link">{{ t('extensions.tabAll') }}</RouterLink>
      </template>
    </section>
  </div>

  <ExtCategoryDialog v-model:open="dialogOpen" :category="editing" @saved="onSaved" />
  <ConfirmDialog
    v-model:open="removal.open.value"
    :title="t('extensions.deleteSetTitle', { name: removal.shown.value?.name ?? '' })"
    :description="t('extensions.deleteSetText', removal.shown.value?.profile_count ?? 0)"
    :confirm-label="t('extensions.deleteSetButton')"
    :busy="removal.busy.value"
    danger
    @confirm="removal.confirm"
  />
</template>

<style scoped>
.layout {
  display: flex;
  align-items: flex-start;
  gap: 24px;
  flex-grow: 1;
  min-height: 0;
}

.placeholder {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 12px;
  flex-grow: 1;
  padding: 24px 16px;
}
</style>
