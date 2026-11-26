<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FieldInfo, Issue } from '../../../api/types'
import UiField from '../../../components/UiField.vue'
import UiSegmented from '../../../components/UiSegmented.vue'
import UiSelect from '../../../components/UiSelect.vue'
import FieldControl from './FieldControl.vue'
import IntRangeField from './fields/IntRangeField.vue'
import VarText from './fields/VarText.vue'

const props = defineProps<{ fields: FieldInfo[]; config?: Record<string, unknown>; vars: string[]; errors: Issue[]; warnings: Issue[] }>()
const emit = defineEmits<{ update: [name: string, value: unknown] }>()
const { t } = useI18n()
const field = (name: string) => props.fields.find((field) => field.name === name)
const value = (name: string) => props.config?.[name] ?? field(name)?.default
const error = (name: string) => props.errors.find((issue) => issue.field === name)?.message
const warning = (name: string) => props.warnings.find((issue) => issue.field === name)?.message
const position = computed(() => String(value('position') ?? 'bottom'))
const pixels = computed(() => {
  const current = value('pixels')
  return typeof current === 'number' || typeof current === 'string' ? current : undefined
})
const positions = computed(() => ['top', 'middle', 'bottom'].map((value) => ({ value, label: t(`automation.choices.position.${value}`) })))
</script>

<template>
  <UiField :label="t('automation.editor.scroll.area')" :error="error('area')" :hint="warning('area')">
    <UiSegmented :model-value="String(value('area') ?? 'page')" :aria-label="t('automation.editor.scroll.area')" class="scroll-area"
      :options="[{ value: 'page', label: t('automation.choices.page') }, { value: 'target', label: t('automation.editor.w.selector') }]"
      @update:model-value="emit('update', 'area', $event)" />
  </UiField>
  <FieldControl v-if="value('area') === 'target' && field('target')" :field="field('target')!" :value="config?.target" :vars="vars" :flows="[]"
    :error="error('target')" :warning="warning('target')" @update="emit('update', 'target', $event)" />
  <UiField :label="t('automation.fields.position')" :error="error('position') || error('pixels')" :hint="warning('position') || warning('pixels')">
    <div class="scroll-position">
      <UiSelect :model-value="position === 'pixels' ? 'pixels' : 'preset'" :aria-label="t('automation.editor.scroll.positionMode')"
        :options="[{ value: 'preset', label: t('automation.fields.position') }, { value: 'pixels', label: t('automation.choices.pixels') }]"
        @update:model-value="emit('update', 'position', $event === 'pixels' ? 'pixels' : 'bottom')" />
      <VarText v-if="position === 'pixels'" id="f-pixels" :model-value="pixels" :vars="vars" numeric :invalid="!!error('pixels')"
        :aria-label="t('automation.choices.pixels')" @update:model-value="emit('update', 'pixels', $event)" />
      <UiSelect v-else id="f-position" :model-value="position" :options="positions" :invalid="!!error('position')"
        :aria-label="t('automation.editor.scroll.positionValue')" @update:model-value="emit('update', 'position', $event)" />
    </div>
  </UiField>
  <UiField :label="t('automation.editor.scroll.kind')" :error="error('kind')" :hint="warning('kind')">
    <div class="scroll-kind" role="radiogroup" :aria-label="t('automation.editor.scroll.kind')">
      <label v-for="kind in ['smooth', 'auto']" :key="kind">
        <input type="radio" name="scroll-kind" :value="kind" :checked="value('kind') === kind"
          @change="emit('update', 'kind', kind)" />
        {{ t(kind === 'auto' ? 'automation.editor.scroll.auto' : 'automation.choices.smooth') }}
      </label>
    </div>
  </UiField>
  <section v-if="value('kind') === 'smooth'" class="scroll-speed" aria-labelledby="scroll-speed-title">
    <h3 id="scroll-speed-title">{{ t('automation.editor.scroll.speed') }}</h3>
    <p class="hint">{{ t('automation.editor.scroll.rangeHint') }}</p>
    <div v-for="name in ['step_px', 'step_ms']" :key="name" class="scroll-step">
      <p :id="`scroll-${name}-label`">{{ t(`automation.editor.scroll.${name}`) }}</p>
      <IntRangeField :id="`f-${name}`" :value="value(name)" :vars="vars" range-only :invalid="!!error(name)"
        :aria-labelledby="`scroll-${name}-label`" @update="emit('update', name, $event)" />
      <span class="hint">{{ t(`automation.editor.scroll.${name}Unit`) }}</span>
      <p v-if="error(name)" class="error-text">{{ error(name) }}</p>
      <p v-else-if="warning(name)" class="hint">{{ warning(name) }}</p>
    </div>
  </section>
</template>

<style scoped>
.scroll-area { width: 210px; max-width: 100%; }
.scroll-area :deep(.segment[data-state='on']) { color: var(--accent-ink); }
.scroll-position { display: grid; grid-template-columns: 100px minmax(0, 1fr); gap: 8px; }
.scroll-kind { display: flex; gap: 24px; }
.scroll-kind label { display: inline-flex; align-items: center; gap: 6px; cursor: pointer; }
.scroll-kind input { width: 14px; height: 14px; margin: 0; accent-color: var(--accent); }
.scroll-speed { display: flex; flex-direction: column; gap: 8px; }
.scroll-speed h3 { margin: 0; font-size: 13px; line-height: 18px; font-weight: 500; color: var(--text-2); }
.scroll-step { display: flex; flex-direction: column; gap: 6px; padding: 12px; border-radius: 8px; background: color-mix(in srgb, var(--muted-bg) 60%, var(--surface)); }
.scroll-step p { margin: 0; line-height: 20px; }
</style>
