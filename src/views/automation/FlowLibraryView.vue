<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave, onBeforeRouteUpdate, useRoute, useRouter } from 'vue-router'
import type { FlowDoc } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import UiButton from '../../components/UiButton.vue'
import { flowTemplates, templateCategories } from '../../lib/flowTemplates'
import { notify } from '../../lib/notify'
import { formDefault, parseStrictNumber } from '../../lib/paramDefault'
import { useAutomationStore } from '../../stores/automation'
import AutomationHeader from './AutomationHeader.vue'
import FlowTemplateCover from './FlowTemplateCover.vue'
import ParamField from './ParamField.vue'

const { t, locale } = useI18n()
const route = useRoute()
const router = useRouter()
const automation = useAutomationStore()
const page = ref<HTMLElement>()
const importing = ref(false)
const values = ref<Record<string, string | boolean>>({})
const errors = ref<Record<string, string>>({})
const language = computed(() => locale.value === 'en' ? 'en' : 'ru')
const templates = computed(() => flowTemplates.map(template => ({
  ...template, flow: JSON.parse(template.sources[language.value]) as FlowDoc,
  summary: template.summary[language.value], instructions: template.instructions[language.value],
})))
const selected = computed(() => templates.value.find(template => template.id === route.params.templateId))
const search = computed({
  get: () => typeof route.query.q === 'string' ? route.query.q : '',
  set: (q: string) => { void router.replace({ query: { ...route.query, q: q || undefined } }) },
})
const category = computed(() => templateCategories.find(id => id === route.query.category) ?? 'all')
const filtered = computed(() => {
  const words = search.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  return templates.value.filter(template => {
    const text = [template.id, template.flow.name, template.summary, t(`automation.library.categories.${template.category}`)]
      .join(' ').toLocaleLowerCase()
    return (category.value === 'all' || category.value === template.category) && words.every(word => text.includes(word))
  })
})
const catalogLocation = computed(() => ({ path: '/automation/library', query: route.query }))

watch(() => route.params.templateId, async (id, previous) => {
  values.value = Object.fromEntries((selected.value?.flow.variables ?? []).map(v => [v.name, formDefault(v)]))
  errors.value = {}
  await nextTick()
  if (id) {
    page.value?.querySelector<HTMLElement>('h2')?.focus()
    if (page.value) page.value.scrollTop = 0
  } else if (previous) {
    Array.from(page.value?.querySelectorAll<HTMLElement>('[data-template-id]') ?? [])
      .find(tile => tile.dataset.templateId === previous)?.focus()
  }
}, { immediate: true })

// A pending copy must finish before leaving, otherwise its completion could redirect a different screen.
onBeforeRouteLeave(() => !importing.value)
onBeforeRouteUpdate(() => !importing.value)

async function useTemplate() {
  if (importing.value || !selected.value) return
  const flow = JSON.parse(selected.value.sources[language.value]) as FlowDoc
  const bad: Record<string, string> = {}
  for (const variable of flow.variables ?? []) {
    const raw = values.value[variable.name]
    if (raw === formDefault(variable)) continue
    if (variable.type === 'number') {
      const number = parseStrictNumber(String(raw))
      if (number === null || !Number.isFinite(number)) bad[variable.name] = t('automation.taskDialog.expectNumber')
      else variable.default = number
    } else if (variable.type === 'array' || variable.type === 'object') {
      try {
        const value: unknown = JSON.parse(String(raw))
        if (variable.type === 'array' ? !Array.isArray(value) : !value || typeof value !== 'object' || Array.isArray(value)) throw new Error('type')
        variable.default = value
      } catch { bad[variable.name] = t('automation.taskDialog.expectJson') }
    } else variable.default = raw
  }
  errors.value = bad
  if (Object.keys(bad).length) {
    await nextTick()
    page.value?.querySelector<HTMLElement>('.invalid')?.focus()
    return
  }
  importing.value = true
  try {
    const copy = await automation.importTemplate(JSON.stringify(flow))
    importing.value = false
    await router.push(`/automation/flows/${copy.id}`)
  } catch (error) {
    notify.error(error)
  } finally { importing.value = false }
}

function resetFilters() {
  void router.replace({ query: {} })
}
</script>

<template>
  <main ref="page" class="page list-page automation-library">
    <AutomationHeader />
    <template v-if="!route.params.templateId">
      <div class="catalog-heading">
        <div>
          <h2>{{ t('automation.library.title') }}</h2>
          <p class="hint">{{ t('automation.library.subtitle') }}</p>
        </div>
        <label class="library-search input">
          <AppIcon name="search" />
          <span class="sr-only">{{ t('automation.library.search') }}</span>
          <input v-model="search" type="search" :placeholder="t('automation.library.search')" />
        </label>
      </div>
      <div class="catalog-filters">
        <div class="categories" role="group" :aria-label="t('automation.library.categoryLabel')">
          <button v-for="id in ['all', ...templateCategories]" :key="id" type="button" class="category"
            :data-category="id" :aria-pressed="category === id"
            @click="router.replace({ query: { ...route.query, category: id === 'all' ? undefined : id } })">
            {{ t(`automation.library.categories.${id}`) }}
          </button>
        </div>
        <p class="hint result-count" role="status">{{ t('automation.library.count', filtered.length) }}</p>
      </div>
      <div v-if="filtered.length" class="library-grid">
        <RouterLink v-for="template in filtered" :key="template.id" class="library-tile" :data-template-id="template.id"
          :to="{ path: `/automation/library/${template.id}`, query: route.query }"
          :aria-labelledby="`template-title-${template.id}`">
          <FlowTemplateCover />
          <div class="tile-body">
            <h3 :id="`template-title-${template.id}`">{{ template.flow.name }}</h3>
            <p>{{ template.summary }}</p>
            <div class="tile-footer">
              <span class="template-category">{{ t(`automation.library.categories.${template.category}`) }}</span>
              <AppIcon name="next" />
            </div>
          </div>
        </RouterLink>
      </div>
      <div v-else class="library-empty">
        <AppIcon name="search" :size="28" />
        <h3>{{ t('automation.library.empty') }}</h3>
        <p class="hint">{{ t('automation.library.emptyHint') }}</p>
        <UiButton @click="resetFilters">{{ t('automation.library.reset') }}</UiButton>
      </div>
    </template>
    <template v-else>
      <RouterLink class="library-back" :to="catalogLocation" :aria-disabled="importing || undefined">
        <AppIcon name="prev" />{{ t('automation.library.back') }}
      </RouterLink>
      <form v-if="selected" class="template-detail" @submit.prevent="useTemplate">
        <header class="detail-heading">
          <div>
            <h2 tabindex="-1">{{ selected.flow.name }}</h2>
            <p class="hint">{{ t(`automation.library.categories.${selected.category}`) }}</p>
          </div>
          <UiButton type="submit" variant="primary" icon="plus" class="template-use" :loading="importing">
            {{ t('automation.library.use') }}
          </UiButton>
        </header>
        <div class="detail-columns">
          <div class="template-guide">
            <FlowTemplateCover />
            <section :aria-label="t('automation.library.description')">
              <h3>{{ t('automation.library.description') }}</h3>
              <p>{{ selected.flow.remark }}</p>
            </section>
            <section class="template-instructions" :aria-label="t('automation.library.instructions')">
              <h3>{{ t('automation.library.instructions') }}</h3>
              <ol><li v-for="step in selected.instructions" :key="step">{{ step }}</li></ol>
            </section>
          </div>
          <section class="template-parameters" :aria-label="t('automation.library.parameters')">
            <h3>{{ t('automation.library.parameters') }}</h3>
            <p class="hint">{{ t('automation.library.parametersHint') }}</p>
            <fieldset :disabled="importing">
              <legend class="sr-only">{{ t('automation.library.parameters') }}</legend>
              <ParamField v-for="parameter in selected.flow.variables ?? []" :key="parameter.name"
                v-model="values[parameter.name]!" :decl="parameter" :error="errors[parameter.name]"
                />
            </fieldset>
            <p class="hint copy-hint">{{ t('automation.library.hint') }}</p>
          </section>
        </div>
      </form>
      <div v-else class="library-empty">
        <h2 tabindex="-1">{{ t('automation.library.notFound') }}</h2>
        <p class="hint">{{ t('automation.library.notFoundHint') }}</p>
      </div>
    </template>
  </main>
</template>

<style scoped>
.automation-library { gap: 24px; }
h2, h3, p { margin: 0; }
h2 { font-size: 24px; line-height: 32px; font-weight: 600; overflow-wrap: anywhere; }
h3 { font-size: 15px; line-height: 22px; font-weight: 600; }
.catalog-heading, .detail-heading { display: flex; justify-content: space-between; align-items: center; gap: 20px; }
.catalog-heading .hint, .detail-heading .hint { margin-top: 6px; }
.library-search { display: flex; align-items: center; gap: 10px; width: 360px; max-width: 100%; flex-shrink: 0; color: var(--text-2); cursor: text; }
.library-search input { width: 100%; min-width: 0; border: 0; outline: 0; background: transparent; }
.library-search:focus-within { outline: 2px solid var(--focus); outline-offset: 2px; }
.catalog-filters { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
.categories { display: flex; flex-wrap: wrap; gap: 8px; }
.category { min-height: 44px; padding: 0 16px; border: 0; border-radius: var(--radius-control); background: transparent; color: var(--text-2); cursor: pointer; }
.category:hover { background: var(--muted-bg); color: var(--text); }
.category[aria-pressed='true'] { background: var(--accent-soft); color: var(--accent-ink); font-weight: 500; }
.category:focus-visible, .library-tile:focus-visible, .library-back:focus-visible { outline: 2px solid var(--focus); outline-offset: 3px; }
.result-count { white-space: nowrap; }
.library-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 24px; align-items: stretch; }
.library-tile { display: flex; flex-direction: column; border: 1px solid var(--border); border-radius: var(--radius-card); color: var(--text); text-decoration: none; overflow: hidden; min-width: 0; }
.library-tile:hover { border-color: var(--accent); }
.library-tile:active { background: var(--accent-soft); }
.tile-body { display: flex; flex: 1; flex-direction: column; gap: 12px; padding: 20px; }
.tile-body h3 { overflow-wrap: anywhere; }
.tile-body p { color: var(--text-2); line-height: 22px; }
.tile-footer { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-top: auto; padding-top: 8px; color: var(--text-2); }
.template-category { font-size: 13px; line-height: 18px; }
.library-empty { display: flex; flex-direction: column; align-items: center; gap: 12px; padding: 64px 24px; text-align: center; }
.library-empty > .icon { color: var(--text-2); }
.library-empty .btn { margin-top: 8px; }
.library-back { display: inline-flex; gap: 8px; align-items: center; align-self: flex-start; min-height: 44px; text-decoration: none; }
.library-back[aria-disabled='true'] { color: var(--text-2); cursor: wait; }
.template-detail { display: flex; flex-direction: column; gap: 28px; max-width: 1120px; }
.detail-heading { align-items: flex-start; }
.detail-heading > div { min-width: 0; }
.detail-columns { display: grid; grid-template-columns: minmax(0, 1fr) minmax(280px, 0.85fr); gap: 40px; align-items: start; }
.template-guide { display: flex; flex-direction: column; gap: 28px; min-width: 0; }
.template-guide > .template-cover { max-width: 420px; border-radius: var(--radius-card); }
.template-guide section { max-width: 72ch; }
.template-guide h3 { margin-bottom: 12px; }
.template-guide p, .template-instructions li { line-height: 1.65; overflow-wrap: anywhere; }
.template-instructions ol { padding-left: 22px; margin: 0; }
.template-instructions li { padding-left: 4px; }
.template-instructions li + li { margin-top: 16px; }
.template-parameters { border-top: 1px solid var(--border); padding-top: 20px; min-width: 0; }
.template-parameters > .hint { margin-top: 8px; }
fieldset { display: flex; flex-direction: column; gap: 20px; min-width: 0; padding: 24px 0; margin: 0; border: 0; }
.copy-hint { border-top: 1px solid var(--border); padding-top: 16px; }
@media (max-width: 1000px) {
  .catalog-heading { align-items: flex-start; flex-wrap: wrap; }
  .library-search { width: 100%; }
  .detail-columns { gap: 24px; }
}
@media (max-width: 800px) {
  .automation-library { padding: 20px 16px; }
  .detail-columns { grid-template-columns: minmax(0, 1fr); }
  .detail-heading { flex-wrap: wrap; gap: 16px; }
  .library-grid { grid-template-columns: repeat(auto-fill, minmax(min(100%, 240px), 1fr)); gap: 16px; }
}
</style>
