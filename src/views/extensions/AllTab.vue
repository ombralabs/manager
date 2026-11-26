<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { ExtensionRecord } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import ConfirmDialog from '../../components/ConfirmDialog.vue'
import type { SelectOption } from '../../components/types'
import UiButton from '../../components/UiButton.vue'
import UiSegmented from '../../components/UiSegmented.vue'
import { setsWith, toggledItems } from '../../lib/extensions'
import { notify } from '../../lib/notify'
import { fold } from '../../lib/tags'
import { useConfirm } from '../../lib/useConfirm'
import { useExtensionsStore } from '../../stores/extensions'
import AddExtensionDialog from './AddExtensionDialog.vue'
import ExtensionCard from './ExtensionCard.vue'

const { t } = useI18n()
const store = useExtensionsStore()

const addOpen = ref(false)
/** «Добавить расширение» в шапке экрана. */
defineExpose({ add: () => (addOpen.value = true) })

type Filter = 'all' | 'on' | 'off'
const query = ref('')
const filter = ref<Filter>('all')
const onCount = computed(() => store.extensions.filter((ext) => store.enabledIds.has(ext.id)).length)
const filterOptions = computed<SelectOption<Filter>[]>(() => [
  { value: 'all', label: t('extensions.filter.all', { n: store.extensions.length }) },
  { value: 'on', label: t('extensions.filter.on', { n: onCount.value }) },
  { value: 'off', label: t('extensions.filter.off', { n: store.extensions.length - onCount.value }) },
])
/** Поиск — вхождение в имя без учёта регистра и «ё/е»; фильтр — состав набора 0. */
const shown = computed(() => {
  const needle = fold(query.value)
  return store.extensions.filter(
    (ext) => (filter.value === 'all' || (filter.value === 'on') === store.enabledIds.has(ext.id)) && fold(ext.name).includes(needle),
  )
})

/** Состав набора 0 сохраняется: второй щелчок не уйдёт со старым составом. */
const saving = ref(false)
async function setEnabled(id: string, on: boolean) {
  const general = store.categoryById(0)
  if (!general || saving.value) return
  saving.value = true
  try {
    await store.setItems(0, toggledItems(store.extensions, general.extension_ids, id, on))
  } catch (e) {
    notify.error(e)
  } finally {
    saving.value = false
  }
}

/** Расширение, которое сейчас обновляется из Web Store. */
const updating = ref<string | null>(null)
async function update(ext: ExtensionRecord) {
  updating.value = ext.id
  try {
    const record = await core.updateExtension(ext.id)
    const key = record.version === ext.version ? 'extensions.noUpdate' : 'extensions.updated'
    const title = t(key, { name: record.name, version: record.version })
    // Новая версия получила право `proxy`/`privacy` — как и при добавлении, предупреждаем.
    if (record.gateway_risk && !ext.gateway_risk) notify.warning(title, t('extensions.riskText'))
    else notify.info(title)
    await store.refresh().catch(notify.error)
  } catch (e) {
    notify.error(e)
  } finally {
    updating.value = null
  }
}

const removal = useConfirm(async (ext: ExtensionRecord) => {
  await core.removeExtension(ext.id)
  notify.info(t('extensions.removed', { name: ext.name }))
  await store.refresh().catch(notify.error)
})
/** Подтверждение называет наборы, из которых уйдёт расширение; набор 0 — это «включено», его не называем. */
const removalText = computed(() => {
  const ext = removal.shown.value
  if (!ext) return ''
  const sets = setsWith(store.categories, ext.id)
  return sets.length > 0 ? t('extensions.removeInSets', { sets: sets.join(', ') }) : t('extensions.removeKeepsData')
})

const openPage = (ext: ExtensionRecord) => core.openStorePage(ext.id).catch(notify.error)
const openStore = () => core.openStoreSearch('').catch(notify.error)
</script>

<template>
  <div class="toolbar">
    <label class="search input">
      <AppIcon name="search" />
      <span class="sr-only">{{ t('extensions.search') }}</span>
      <input v-model="query" type="search" :placeholder="t('extensions.searchPlaceholder')" />
    </label>
    <UiSegmented v-model="filter" :options="filterOptions" :aria-label="t('extensions.filterLabel')" />
    <UiButton class="store" @click="openStore">{{ t('extensions.openStore') }}<AppIcon name="external" :size="16" /></UiButton>
  </div>
  <p class="note"><AppIcon name="info" />{{ t('extensions.infoAll') }}</p>
  <div v-if="shown.length > 0 || store.pending.length > 0" class="grid">
    <ExtensionCard
      v-for="ext in shown"
      :key="ext.id"
      :ext="ext"
      :enabled="store.enabledIds.has(ext.id)"
      :sets="setsWith(store.categories, ext.id)"
      :saving="saving"
      :updating="updating"
      @toggle="setEnabled(ext.id, $event)"
      @update="update(ext)"
      @remove="removal.ask(ext)"
      @open-store="openPage(ext)"
    />
    <ExtensionCard v-for="item in store.pending" :key="`pending-${item.key}`" :pending="item" />
  </div>
  <div v-else-if="store.loaded && store.extensions.length === 0" class="empty card">
    <div class="hint">{{ t('extensions.empty') }}</div>
    <UiButton variant="primary" icon="plus" @click="addOpen = true">{{ t('extensions.addExtension') }}</UiButton>
  </div>
  <p v-else-if="store.loaded" class="hint nothing">{{ t('extensions.nothingFound') }}</p>

  <ConfirmDialog
    v-model:open="removal.open.value"
    :title="t('extensions.removeTitle', { name: removal.shown.value?.name ?? '' })"
    :description="removalText"
    :confirm-label="t('common.delete')"
    :busy="removal.busy.value"
    danger
    @confirm="removal.confirm"
  />
  <AddExtensionDialog v-model:open="addOpen" />
</template>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.search {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 320px;
  color: var(--text-2);
  cursor: text;
}

.search input {
  flex-grow: 1;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
}

.store {
  margin-left: auto;
}

/* Две колонки карточек; прокручивается сетка, шапка экрана на месте. */
.grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  align-content: start;
  gap: 16px;
  flex-grow: 1;
  min-height: 0;
  overflow: auto;
}

.empty {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 12px;
  padding: 24px 16px;
}

.nothing {
  margin: 0;
}
</style>
