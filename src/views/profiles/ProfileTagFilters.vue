<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import AppIcon from '../../components/AppIcon.vue'
import { fold, sameTag } from '../../lib/tags'
import { useGroupsStore } from '../../stores/groups'
import { useProfilesStore } from '../../stores/profiles'

const { t, locale } = useI18n()
const profiles = useProfilesStore()
const groups = useGroupsStore()
const tags = computed(() => {
  const unique = new Map<string, string>()
  for (const name of [...groups.tags.map((tag) => tag.name), ...profiles.rows.flatMap((row) => row.tags), ...profiles.filters.tags]) {
    if (!unique.has(fold(name))) unique.set(fold(name), name)
  }
  // Частота по всем профилям: выбор фильтра не переставляет кнопки под курсором.
  const usage = new Map(groups.tags.map((tag) => [fold(tag.name), tag.profile_count]))
  const compareNames = new Intl.Collator(locale.value, { numeric: true, sensitivity: 'base' }).compare
  return [...unique.values()].sort((a, b) => (usage.get(fold(b)) ?? 0) - (usage.get(fold(a)) ?? 0) || compareNames(a, b))
})
const selected = (tag: string) => profiles.filters.tags.some((name) => sameTag(name, tag))
const count = (tag: string) => profiles.tagFacets.counts.get(fold(tag)) ?? 0
</script>

<template>
  <div class="profile-tag-filters" role="group" :aria-label="t('profiles.tagFilters.label')">
    <button type="button" class="tag-filter" data-tag-filter="untagged" :aria-pressed="profiles.filters.untagged" :disabled="!profiles.filters.untagged && (profiles.filters.tags.length > 0 || profiles.tagFacets.untagged === 0)" :title="t('profiles.tagFilters.matches', { n: profiles.tagFacets.untagged })" @click="profiles.toggleUntagged()">
      <AppIcon v-if="profiles.filters.untagged" name="check" :size="12" />{{ t('profiles.tagFilters.untagged') }}
    </button>
    <span v-if="tags.length" class="tag-filter-divider" aria-hidden="true" />
    <button v-for="tag in tags" :key="fold(tag)" type="button" class="tag-filter" :data-tag-filter="fold(tag)" :aria-pressed="selected(tag)" :disabled="!selected(tag) && (profiles.filters.untagged || count(tag) === 0)" :title="`${tag} · ${t('profiles.tagFilters.matches', { n: count(tag) })}`" @click="profiles.toggleTag(tag)">
      <AppIcon v-if="selected(tag)" name="check" :size="12" /><span>{{ tag }}</span>
    </button>
    <button v-if="profiles.hasTagFilters" type="button" class="tag-filters-clear link" @click="profiles.clearTagFilters()">{{ t('profiles.tagFilters.clear') }}</button>
  </div>
</template>

<style scoped>
.profile-tag-filters { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; flex-shrink: 0; min-width: 0; max-height: 100px; padding: 2px; margin: -2px; overflow-y: auto; }
.tag-filter-divider { align-self: center; flex: 0 0 1px; height: 18px; margin: 0 8px; background: var(--border-strong); }
.tag-filter { display: inline-flex; align-items: center; justify-content: center; gap: 4px; min-width: 32px; min-height: 28px; max-width: 220px; padding: 4px 8px; border: 1px solid var(--border-strong); border-radius: 6px; background: var(--surface); color: var(--text-2); font-size: 12px; line-height: 18px; cursor: pointer; }
.tag-filter > span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tag-filter > .icon { flex-shrink: 0; }
.tag-filter:not(:disabled):hover { background: var(--muted-bg); color: var(--text); }
.tag-filter[aria-pressed='true'] { border-color: var(--accent); background: var(--accent-soft); color: var(--accent-ink); }
.tag-filter[aria-pressed='true']:hover { background: var(--accent-soft); color: var(--accent-ink); }
.tag-filter:disabled { border-color: var(--border); color: var(--text-2); opacity: 0.45; cursor: default; }
.tag-filter:focus-visible, .tag-filters-clear:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.tag-filters-clear { flex-shrink: 0; min-height: 28px; margin-left: 2px; padding: 4px 0; font-size: 12px; }
</style>
