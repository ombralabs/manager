<script setup lang="ts">
import { computed, nextTick, ref, useId, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SortKey } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import UiButton from '../../components/UiButton.vue'
import type { OptionalColumn } from '../../lib/columns'
import { useProfilesStore } from '../../stores/profiles'

const props = defineProps<{ hidden: OptionalColumn[] }>()
const { t } = useI18n()
const profiles = useProfilesStore()
const searchOpen = ref(profiles.filters.search !== '')
const searchInput = ref<HTMLInputElement | null>(null)
const searchToggle = ref<InstanceType<typeof UiButton> | null>(null)
const searchId = useId()
watch(() => profiles.filters.search, (value) => { if (value !== '') searchOpen.value = true })

async function toggleSearch() {
  searchOpen.value = !searchOpen.value
  if (!searchOpen.value) profiles.filters.search = ''
  await nextTick()
  if (searchOpen.value) searchInput.value?.focus()
  else (searchToggle.value?.$el as HTMLButtonElement | undefined)?.focus()
}

/** Сортировка по ранее скрытой колонке остаётся видимой и сбрасывается одним действием. */
const OPTIONAL_SORT: Partial<Record<SortKey, OptionalColumn>> = { serial: 'serial', created: 'created', last_opened: 'last_opened' }
const hiddenSort = computed(() => {
  if (profiles.filters.sort === 'serial' && !profiles.filters.descending) return null
  const column = OPTIONAL_SORT[profiles.filters.sort]
  return column !== undefined && props.hidden.includes(column) ? column : null
})
const sortDirection = computed(() =>
  profiles.filters.descending ? t('profiles.sortBadge.descending') : t('profiles.sortBadge.ascending'),
)
</script>

<template>
  <div class="profile-tools">
    <div v-if="hiddenSort" class="sort-badge" data-test="sort-badge">
      <span>{{ t('profiles.sortBadge.label', { column: t(`columns.${hiddenSort}`) }) }}</span>
      <AppIcon :name="profiles.filters.descending ? 'sortDesc' : 'sortAsc'" :size="14" />
      <span class="sr-only">{{ sortDirection }}</span>
      <button type="button" class="badge-reset" :aria-label="t('profiles.sortBadge.reset')" :title="t('profiles.sortBadge.reset')" @click="profiles.resetSort()">
        <AppIcon name="close" :size="12" />
      </button>
    </div>
    <div class="profile-search" :class="{ expanded: searchOpen }" role="search" :aria-label="t('profiles.searchLabel')">
      <UiButton ref="searchToggle" size="icon" icon="search" class="search-toggle"
        :aria-label="t(searchOpen ? 'profiles.hideSearch' : 'profiles.searchLabel')" :title="t(searchOpen ? 'profiles.hideSearch' : 'profiles.searchLabel')"
        :aria-expanded="searchOpen" :aria-controls="searchOpen ? searchId : undefined" @click="toggleSearch" />
      <input v-if="searchOpen" :id="searchId" ref="searchInput" v-model="profiles.filters.search" type="search"
        :aria-label="t('profiles.searchLabel')" :placeholder="t('profiles.searchPlaceholder')" autocomplete="off" @keydown.esc.prevent.stop="toggleSearch" />
    </div>
  </div>
</template>

<style scoped>
.profile-tools { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; min-width: 0; max-width: 100%; }
.profile-search { display: flex; align-items: center; min-width: 0; max-width: 100%; height: var(--control-h); border: 1px solid var(--border-strong); border-radius: var(--radius-control); background: var(--surface); }
.profile-search:focus-within { border-color: var(--focus); }
.profile-search .search-toggle { width: calc(var(--control-h) - 2px); height: calc(var(--control-h) - 2px); border: 0; border-radius: calc(var(--radius-control) - 1px); }
.profile-search.expanded .search-toggle { color: var(--accent-ink); }
.profile-search input { width: 240px; max-width: calc(100vw - 160px); min-width: 0; height: 100%; padding: 0 10px 0 2px; border: 0; outline: 0; background: transparent; color: var(--text); }
.sort-badge { display: inline-flex; flex-shrink: 0; align-items: center; gap: 4px; height: 28px; padding: 0 4px 0 10px; border-radius: 999px; background: var(--accent-soft); font-size: 13px; white-space: nowrap; }
.badge-reset { display: inline-flex; padding: 3px; border: 0; border-radius: 50%; background: transparent; color: var(--text-2); cursor: pointer; }
.badge-reset:hover { color: var(--text); }
.badge-reset:focus-visible { outline: 2px solid var(--focus); }
</style>
