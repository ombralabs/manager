<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'

defineProps<{ subtitle?: string }>()
const { t } = useI18n()
const route = useRoute()
</script>

<template>
  <header class="head">
    <div class="title-group">
      <h1 class="page-title">{{ t('automation.title') }}</h1>
      <div v-if="subtitle" class="hint">{{ subtitle }}</div>
    </div>
    <div class="head-actions"><slot name="actions" /></div>
  </header>
  <nav class="tabs" :aria-label="t('automation.tabs')">
    <RouterLink to="/automation/flows" class="tab">{{ t('automation.tabFlows') }}</RouterLink>
    <RouterLink to="/automation/library" class="tab"
      :aria-current="route.path.startsWith('/automation/library') ? 'page' : undefined">{{ t('automation.library.title') }}</RouterLink>
    <RouterLink to="/automation/tasks" class="tab">{{ t('automation.tabTasks') }}</RouterLink>
    <RouterLink to="/automation/journal" class="tab">{{ t('automation.tabJournal') }}</RouterLink>
  </nav>
</template>

<style scoped>
.head-actions {
  display: flex;
  gap: 10px;
}

.tabs {
  display: flex;
  gap: 0 28px;
  flex-shrink: 0;
  min-height: 44px;
  flex-wrap: wrap;
  border-bottom: 1px solid var(--border);
}

.tab {
  display: inline-flex;
  align-items: center;
  height: 44px;
  margin-bottom: -1px;
  border-bottom: 2px solid transparent;
  color: var(--text-2);
  font-weight: 500;
  text-decoration: none;
  white-space: nowrap;
}

.tab[aria-current='page'] {
  border-bottom-color: var(--text);
  color: var(--text);
  font-weight: 600;
}

.tab:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}
</style>
