<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import UiButton from '../../components/UiButton.vue'
import UiSelect from '../../components/UiSelect.vue'
import { PAGE_SIZES, type PageSize } from '../../stores/profiles'

const props = defineProps<{ page: number; pageCount: number; from: number; to: number; total: number; pageSize: PageSize }>()
const emit = defineEmits<{ page: [page: number]; pageSize: [size: PageSize] }>()
const { t } = useI18n()

const pages = computed(() => {
  const indices = new Set([0, props.pageCount - 1, props.page - 1, props.page, props.page + 1])
  if (props.page < 2) indices.add(2)
  if (props.page >= props.pageCount - 2) indices.add(props.pageCount - 3)
  const sorted = [...indices].filter(n => n >= 0 && n < props.pageCount).sort((a, b) => a - b)
  const result: (number | string)[] = []
  sorted.forEach((n, i) => {
    if (i > 0 && n - sorted[i - 1]! > 1) result.push(`gap-${n}`)
    result.push(n)
  })
  return result
})
const sizeOptions = PAGE_SIZES.map((size) => ({ value: String(size), label: String(size) }))
const sizeModel = computed({
  get: () => String(props.pageSize),
  set: (value: string) => {
    const size = PAGE_SIZES.find((candidate) => String(candidate) === value)
    if (size !== undefined) emit('pageSize', size)
  },
})
</script>

<template>
  <footer class="pager">
    <span class="range">{{ t('profiles.pager.range', { from, to, total }) }}</span>
    <div class="spacer" />
    <span class="hint">{{ t('profiles.pager.pageSize') }}</span>
    <div class="size">
      <UiSelect v-model="sizeModel" :options="sizeOptions" :aria-label="t('profiles.pager.pageSize')" mono />
    </div>
    <UiButton
      size="icon"
      variant="ghost"
      icon="prev"
      :disabled="page === 0"
      :aria-label="t('profiles.pager.prev')"
      :title="t('profiles.pager.prev')"
      @click="emit('page', page - 1)"
    />
    <nav class="page-links" :aria-label="t('profiles.pager.page', { page: page + 1, pages: pageCount })">
      <template v-for="item in pages" :key="item">
        <button v-if="typeof item === 'number'" type="button" :class="['page-link', { current: item === page }]" :aria-label="t('profiles.pageNumber', { n: item + 1 })" :aria-current="item === page ? 'page' : undefined" @click="emit('page', item)">{{ item + 1 }}</button>
        <span v-else class="ellipsis">…</span>
      </template>
    </nav>
    <UiButton
      size="icon"
      variant="ghost"
      icon="next"
      :disabled="page >= pageCount - 1"
      :aria-label="t('profiles.pager.next')"
      :title="t('profiles.pager.next')"
      @click="emit('page', page + 1)"
    />
  </footer>
</template>

<style scoped>
.pager {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 10px;
  min-height: 76px;
  padding: 12px 26px;
  border-top: 1px solid var(--border);
}

.range {
  font-size: 13px;
  color: var(--text-2);
}

.spacer {
  flex-grow: 1;
}

.size {
  width: 76px;
}

.page-links {
  display: flex;
  align-items: center;
  gap: 3px;
}

.page-link {
  width: 34px;
  height: 36px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text);
  font-size: 13px;
  cursor: pointer;
}

.page-link:hover {
  background: var(--muted-bg);
}

.page-link.current {
  background: var(--sidebar-active);
  color: var(--accent-ink);
  font-weight: 500;
}

.page-link:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 1px;
}

.ellipsis {
  padding-inline: 5px;
  color: var(--text-2);
}

@media (max-width: 1280px) {
  .pager > .hint {
    display: none;
  }
  .pager {
    gap: 5px;
  }
}
</style>
