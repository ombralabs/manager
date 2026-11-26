<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ExtCategory, ExtensionRecord } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import UiButton from '../../components/UiButton.vue'

/**
 * Наборы вкладки «Наборы» без набора 0 (холст «Расширения: наборы»). `library` — для числа расширений в строке:
 * id, которых нет в библиотеке, не считаются. `risky` — id наборов, где есть расширение с `gateway_risk`.
 */
const props = defineProps<{ sets: ExtCategory[]; selected: number | null; library: ExtensionRecord[]; risky: ReadonlySet<number> }>()
const emit = defineEmits<{ select: [id: number]; rename: [set: ExtCategory]; remove: [set: ExtCategory] }>()
const { t } = useI18n()

const known = computed(() => new Set(props.library.map((ext) => ext.id)))
const count = (set: ExtCategory) => set.extension_ids.filter((id) => known.value.has(id)).length
</script>

<template>
  <section class="sets card" :aria-label="t('extensions.sets')">
    <div class="card-head"><h2 class="section-title">{{ t('extensions.sets') }}</h2></div>
    <ul v-if="sets.length > 0" class="list">
      <li v-for="set in sets" :key="set.id" :class="['item', { selected: set.id === selected }]" data-test="set-row">
        <button type="button" class="pick" :aria-current="set.id === selected ? 'true' : undefined" @click="emit('select', set.id)">
          <span class="name">
            <span class="name-text">{{ set.name }}</span>
            <span v-if="risky.has(set.id)" class="risk" :title="t('extensions.setRisky')">
              <AppIcon name="alert" :size="14" /><span class="sr-only">{{ t('extensions.setRisky') }}</span>
            </span>
          </span>
          <span class="meta">{{ t('extensions.extensionsCount', count(set)) }} · {{ t('extensions.setProfiles', set.profile_count) }}</span>
        </button>
        <UiButton
          size="icon"
          variant="ghost"
          icon="edit"
          :aria-label="t('extensions.renameSet', { name: set.name })"
          :title="t('extensions.renameTitle')"
          @click="emit('rename', set)"
        />
        <UiButton
          size="icon"
          variant="ghost"
          icon="trash"
          class="danger-icon"
          :aria-label="t('extensions.deleteSet', { name: set.name })"
          :title="t('extensions.deleteSetButton')"
          @click="emit('remove', set)"
        />
      </li>
    </ul>
    <p v-else class="hint empty">{{ t('extensions.noSets') }}</p>
  </section>
</template>

<style scoped>
/* Размеры — артборд «Расширения: наборы». */
.sets {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  align-self: flex-start;
  width: 360px;
  max-height: 100%;
  overflow: hidden;
}

.card-head {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  height: 56px;
  padding: 0 16px;
  border-bottom: 1px solid var(--border);
}

.list {
  margin: 0;
  padding: 0;
  overflow: auto;
  list-style: none;
}

.item {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 68px;
  padding-right: 10px;
  border-bottom: 1px solid var(--row-border);
}

.item:last-child {
  border-bottom: 0;
}

.item.selected {
  background: var(--accent-soft);
}

.pick {
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 2px;
  flex-grow: 1;
  align-self: stretch;
  min-width: 0;
  padding: 0 0 0 16px;
  border: 0;
  background: none;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.pick:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: -2px;
}

.name {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-weight: 600;
}

.name-text,
.meta {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.meta {
  font-size: 12px;
  line-height: 16px;
  color: var(--text-2);
}

.risk {
  display: inline-flex;
  flex-shrink: 0;
  color: var(--warn-text);
}

.danger-icon {
  color: var(--danger-text);
}

.empty {
  margin: 0;
  padding: 20px 16px;
}
</style>
