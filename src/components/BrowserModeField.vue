<script setup lang="ts">
import { computed, useId } from 'vue'
import { useI18n } from 'vue-i18n'
import UiField from './UiField.vue'
import UiSelect from './UiSelect.vue'

const model = defineModel<boolean | null>({ required: true })
const props = defineProps<{ inherit?: boolean; inheritedHeadless?: boolean }>()
const { t } = useI18n()
const id = useId()
const mode = computed({
  get: () => model.value === null ? 'inherit' : model.value ? 'headless' : 'window',
  set: (value: string) => { model.value = value === 'inherit' ? null : value === 'headless' },
})
const options = computed(() => [
  ...(props.inherit ? [{ value: 'inherit', label: t('automation.browserMode.inherit', {
    mode: t(props.inheritedHeadless ? 'automation.browserMode.headless' : 'automation.browserMode.window'),
  }) }] : []),
  { value: 'window', label: t('automation.browserMode.window') },
  { value: 'headless', label: t('automation.browserMode.headless') },
])
</script>

<template>
  <UiField :label="t('automation.browserMode.title')" :input-id="id" :hint="t('automation.browserMode.hint')" data-test="browser-mode">
    <UiSelect :id="id" v-model="mode" :options="options" :aria-label="t('automation.browserMode.title')" />
  </UiField>
</template>
