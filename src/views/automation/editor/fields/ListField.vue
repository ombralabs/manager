<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import TagInput from '../../../../components/TagInput.vue'
import UiCheckbox from '../../../../components/UiCheckbox.vue'
import { JS_RESERVED, VAR_NAME } from '../../../../lib/flowEdit'

/** `choice_list` — флажки вариантов; `var_in_list`, `js_var_list`, `text_list` — список строк. Пустой список — значение не задано. */
const props = defineProps<{ kind: string; value: unknown; fallback: unknown; options: { value: string; label: string }[]; vars: string[]; id: string; invalid?: boolean }>()
const emit = defineEmits<{ update: [value: unknown]; error: [message: string | null] }>()
const { t } = useI18n()

const list = computed(() => ((Array.isArray(props.value) ? props.value : Array.isArray(props.fallback) ? props.fallback : []) as string[]))
function toggle(option: string, on: boolean) {
  // Порядок — как у вариантов; пустой выбор уходит как [] и показывается ошибкой поля.
  emit('update', props.options.map((o) => o.value).filter((o) => (o === option ? on : list.value.includes(o))))
}
/** Неверные теги остаются видны с ошибкой поля, в процесс не идут (ядро отвергло бы весь список). */
const bad = ref<string[]>([])
function problem(name: string): string | null {
  if (props.kind === 'text_list') return null
  if (!VAR_NAME.test(name)) return t('automation.editor.w.badName', { name })
  return props.kind === 'js_var_list' && JS_RESERVED.includes(name) ? t('automation.editor.w.jsReserved', { name }) : null
}
let sent: unknown
function tags(next: string[]) {
  const ok = next.filter((n) => !problem(n))
  bad.value = next.filter((n) => problem(n))
  emit('error', bad.value.length > 0 ? problem(bad.value[0]!) : null)
  sent = ok.length === 0 ? undefined : ok
  emit('update', sent)
}
// Значение сменилось извне (отмена, повтор) — отвергнутое набрано к прежнему списку: черновик и его ошибка уходят.
// Эхо своей записи и то же содержимое новым объектом (правка другого поля) — не смена.
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)
watch(() => props.value, (v, old) => {
  if (same(v, old) || same(v, sent) || bad.value.length === 0) return
  bad.value = []
  emit('error', null)
})
</script>

<template>
  <div v-if="kind === 'choice_list'" :id="id" class="checks" role="group">
    <label v-for="o in options" :key="o.value" class="check">
      <UiCheckbox :model-value="list.includes(o.value)" :ariaLabel="o.label" @update:model-value="toggle(o.value, $event)" />{{ o.label }}
    </label>
  </div>
  <TagInput v-else :model-value="[...list, ...bad]" :suggestions="kind === 'text_list' ? [] : vars" :input-id="id" :invalid="invalid" @update:model-value="tags" />
</template>

<style scoped>
.checks { display: flex; flex-direction: column; gap: 8px; }
.check { display: flex; align-items: center; gap: 8px; font-size: 13px; cursor: pointer; }
</style>
