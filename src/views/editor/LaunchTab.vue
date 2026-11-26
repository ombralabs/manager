<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FieldError } from '../../api/types'
import ExtRiskNote from '../../components/ExtRiskNote.vue'
import type { SelectOption } from '../../components/types'
import UiField from '../../components/UiField.vue'
import UiSelect from '../../components/UiSelect.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { useFieldErrors } from '../../lib/errorText'
import { setLabel } from '../../lib/extensions'
import type { EditorModel } from '../../lib/profileForm'
import { useExtensionsStore } from '../../stores/extensions'
import BookmarksEditor from './BookmarksEditor.vue'

const model = defineModel<EditorModel>({ required: true })
const props = defineProps<{ errors: FieldError[] }>()
const { t } = useI18n()
const err = useFieldErrors(() => props.errors)
const extensions = useExtensionsStore()
/**
 * Наборы с числом расширений; значения — строкой, как требует `UiSelect`. Набора профиля нет в списке — свой пункт:
 * «…», пока список грузится, и «Набор удалён» после (сохранение покажет ошибку поля). Без него `UiSelect` показал бы номер.
 */
const setOptions = computed<SelectOption[]>(() => {
  const list = extensions.categories.map((set) => ({
    value: String(set.id),
    label: `${setLabel(set)} · ${t('extensions.extensionsCount', set.extension_ids.length)}`,
  }))
  const current = String(model.value.extCategoryId)
  if (list.some((option) => option.value === current)) return list
  return [{ value: current, label: extensions.loaded ? t('editor.launch.extMissing') : '…' }, ...list]
})
const setModel = computed({
  get: () => String(model.value.extCategoryId),
  set: (value: string) => (model.value.extCategoryId = Number(value)),
})
/** Набор профиля; `undefined` — список не загружен или набор удалён (сохранение покажет ошибку поля). */
const currentSet = computed(() => extensions.categoryById(model.value.extCategoryId))
const setNames = computed(() => extensions.extensionsOf(currentSet.value).map((ext) => ext.name).join(', '))
</script>

<template>
  <div class="stack">
    <section class="card section">
      <h2 class="section-title">{{ t('editor.launch.tabsTitle') }}</h2>
      <div class="switch-row">
        <div class="intro">
          <div>{{ t('editor.launch.restoreTabs') }}</div>
          <div class="hint">{{ t('editor.launch.restoreTabsHint') }}</div>
        </div>
        <UiSwitch v-model="model.restoreTabs" :label="t('editor.launch.restoreTabs')" />
      </div>
      <UiField
        :label="t('editor.launch.startUrls')"
        input-id="start-urls"
        :hint="t('editor.launch.startUrlsHint')"
        :error="err('start_urls')"
      >
        <textarea
          id="start-urls"
          v-model="model.startUrls"
          class="input mono"
          :class="{ invalid: err('start_urls') }"
          rows="4"
          spellcheck="false"
          placeholder="https://example.com/"
        />
      </UiField>
    </section>
    <section class="card section">
      <h2 class="section-title">{{ t('editor.launch.bookmarks') }}</h2>
      <div class="hint">{{ t('editor.launch.bookmarksHint') }}</div>
      <BookmarksEditor v-model="model.bookmarks" :error="err('bookmarks')" />
    </section>
    <section class="card section">
      <h2 class="section-title">{{ t('editor.launch.extensions') }}</h2>
      <UiField :label="t('editor.launch.extSet')" input-id="ext-set" :error="err('ext_category_id')">
        <UiSelect
          id="ext-set"
          v-model="setModel"
          :options="setOptions"
          :disabled="!extensions.loaded"
          :invalid="!!err('ext_category_id')"
        />
      </UiField>
      <div v-if="currentSet" class="hint">{{ setNames || t('editor.launch.extNone') }}</div>
      <ExtRiskNote :extensions="extensions.risky(currentSet)" />
      <div class="hint">{{ t('editor.launch.extHint') }}</div>
    </section>
    <section class="card section">
      <h2 class="section-title">{{ t('editor.launch.title') }}</h2>
      <UiField
        :label="t('editor.launch.extraArgs')"
        input-id="extra-args"
        :hint="t('editor.launch.extraArgsHint')"
        :error="err('extra_args')"
      >
        <textarea
          id="extra-args"
          v-model="model.extraArgs"
          class="input mono"
          :class="{ invalid: err('extra_args') }"
          rows="6"
          spellcheck="false"
          placeholder="--disable-sync"
        />
      </UiField>
    </section>
  </div>
</template>

<style scoped>
.stack {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.switch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
}

.intro {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}
</style>
