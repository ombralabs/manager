<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { FieldError } from '../../api/types'
import type { SelectOption } from '../../components/types'
import TagInput from '../../components/TagInput.vue'
import UiButton from '../../components/UiButton.vue'
import UiCheckbox from '../../components/UiCheckbox.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import UiSegmented from '../../components/UiSegmented.vue'
import UiSelect from '../../components/UiSelect.vue'
import { type BulkEditDraft, editDraftEmpty, editDraftErrors, emptyEditDraft, toBulkEditForm } from '../../lib/bulk'
import { showBulkReport } from '../../lib/bulkReport'
import { useFieldErrors } from '../../lib/errorText'
import { fieldsOrToast, notify } from '../../lib/notify'
import { proxyOptions } from '../../lib/proxyOptions'
import { useGroupsStore } from '../../stores/groups'
import { useProfilesStore } from '../../stores/profiles'
import { useProxiesStore } from '../../stores/proxies'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ ids: string[] }>()
const { t } = useI18n()
const profiles = useProfilesStore()
const groups = useGroupsStore()

const draft = ref<BulkEditDraft>(emptyEditDraft())
const errors = ref<FieldError[]>([])
const proxies = useProxiesStore()
const saving = ref(false)
const err = useFieldErrors(() => errors.value)

watch(
  open,
  async (isOpen) => {
    if (!isOpen) return
    draft.value = emptyEditDraft()
    errors.value = []
    try {
      await proxies.refresh()
    } catch (e) {
      notify.error(e)
    }
  },
  { immediate: true },
)

const empty = computed(() => editDraftEmpty(draft.value))
const tagNames = computed(() => groups.tags.map((tag) => tag.name))

const proxyActionOptions = computed<SelectOption<BulkEditDraft['proxyAction']>[]>(() => [
  { value: 'reset', label: t('bulk.edit.proxyReset') },
  { value: 'set', label: t('bulk.edit.proxySet') },
])
const proxyChoices = computed(() => proxyOptions(proxies.rows, t('bulk.create.pickProxy'), t('bulk.create.proxyOff')))
const proxyModel = computed({
  get: () => (draft.value.proxyId === null ? '' : String(draft.value.proxyId)),
  set: (value: string) => {
    draft.value.proxyId = value === '' ? null : Number(value)
  },
})

const yesNoOptions = computed(() => [
  { value: 'yes', label: t('bulk.edit.yes') },
  { value: 'no', label: t('bulk.edit.no') },
])
const restoreModel = computed({
  get: () => (draft.value.restoreTabs ? 'yes' : 'no'),
  set: (value: string) => {
    draft.value.restoreTabs = value === 'yes'
  },
})

/** Ошибочная вкладка или аргумент — ошибка у поля, ядро не меняет ничего. */
async function submit() {
  const local = editDraftErrors(draft.value)
  errors.value = local
  if (local.length > 0) return
  saving.value = true
  try {
    const report = await core.editProfiles(props.ids, toBulkEditForm(draft.value))
    open.value = false
    showBulkReport('edit', report)
    await Promise.all([profiles.refresh(), groups.refresh()])
  } catch (e) {
    fieldsOrToast(e, errors)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <UiDialog
    v-model:open="open"
    :title="t('bulk.edit.title')"
    :description="t('bulk.edit.subtitle', { n: ids.length })"
    :width="560"
    :busy="saving"
  >
    <div class="fields">
      <div class="field-row">
        <label class="toggle">
          <UiCheckbox v-model="draft.enabled.addTags" :ariaLabel="t('bulk.edit.addTags')" />
          {{ t('bulk.edit.addTags') }}
        </label>
        <UiField v-if="draft.enabled.addTags" :error="err('tags')">
          <TagInput v-model="draft.addTags" input-id="bulk-edit-add-tags" :suggestions="tagNames" :invalid="!!err('tags')" />
        </UiField>
      </div>

      <div class="field-row">
        <label class="toggle">
          <UiCheckbox v-model="draft.enabled.removeTags" :ariaLabel="t('bulk.edit.removeTags')" />
          {{ t('bulk.edit.removeTags') }}
        </label>
        <UiField v-if="draft.enabled.removeTags" :error="err('tags')">
          <TagInput
            v-model="draft.removeTags"
            input-id="bulk-edit-remove-tags"
            :suggestions="tagNames"
            :invalid="!!err('tags')"
          />
        </UiField>
      </div>

      <div class="field-row">
        <label class="toggle">
          <UiCheckbox v-model="draft.enabled.note" :ariaLabel="t('bulk.edit.note')" />
          {{ t('bulk.edit.note') }}
        </label>
        <UiField v-if="draft.enabled.note" input-id="bulk-edit-note" :hint="t('bulk.edit.noteHint')">
          <textarea id="bulk-edit-note" v-model="draft.note" class="input" rows="2" />
        </UiField>
      </div>

      <div class="field-row">
        <label class="toggle">
          <UiCheckbox v-model="draft.enabled.proxy" :ariaLabel="t('bulk.edit.gateway')" />
          {{ t('bulk.edit.gateway') }}
        </label>
        <UiField
          v-if="draft.enabled.proxy"
          :hint="draft.proxyAction === 'reset' ? t('bulk.edit.proxyResetHint') : undefined"
          :error="err('proxy_id')"
        >
          <div class="gateway">
            <UiSegmented v-model="draft.proxyAction" :options="proxyActionOptions" :aria-label="t('bulk.edit.gateway')" />
            <template v-if="draft.proxyAction === 'set'">
              <UiSelect
                v-if="proxies.rows.length > 0"
                v-model="proxyModel"
                :options="proxyChoices"
                :aria-label="t('bulk.create.pickProxy')"
                :invalid="!!err('proxy_id')"
                mono
              />
              <div v-else class="hint">{{ t('bulk.create.noProxies') }}</div>
            </template>
          </div>
        </UiField>
      </div>

      <div class="field-row">
        <label class="toggle">
          <UiCheckbox v-model="draft.enabled.startUrls" :ariaLabel="t('bulk.edit.startUrls')" />
          {{ t('bulk.edit.startUrls') }}
        </label>
        <UiField
          v-if="draft.enabled.startUrls"
          input-id="bulk-edit-start-urls"
          :hint="t('bulk.edit.startUrlsHint')"
          :error="err('start_urls')"
        >
          <textarea
            id="bulk-edit-start-urls"
            v-model="draft.startUrls"
            class="input mono"
            :class="{ invalid: err('start_urls') }"
            rows="3"
            spellcheck="false"
          />
        </UiField>
      </div>

      <div class="field-row">
        <label class="toggle">
          <UiCheckbox v-model="draft.enabled.extraArgs" :ariaLabel="t('bulk.edit.extraArgs')" />
          {{ t('bulk.edit.extraArgs') }}
        </label>
        <UiField
          v-if="draft.enabled.extraArgs"
          input-id="bulk-edit-extra-args"
          :hint="t('bulk.edit.extraArgsHint')"
          :error="err('extra_args')"
        >
          <textarea
            id="bulk-edit-extra-args"
            v-model="draft.extraArgs"
            class="input mono"
            :class="{ invalid: err('extra_args') }"
            rows="2"
            spellcheck="false"
          />
        </UiField>
      </div>

      <div class="field-row">
        <label class="toggle">
          <UiCheckbox v-model="draft.enabled.restoreTabs" :ariaLabel="t('bulk.edit.restoreTabs')" />
          {{ t('bulk.edit.restoreTabs') }}
        </label>
        <UiSegmented
          v-if="draft.enabled.restoreTabs"
          v-model="restoreModel"
          :options="yesNoOptions"
          :aria-label="t('bulk.edit.restoreTabs')"
        />
      </div>
    </div>
    <template #footer>
      <div class="footer">
        <div class="buttons">
          <UiButton :disabled="saving" @click="open = false">{{ t('common.cancel') }}</UiButton>
          <UiButton variant="primary" :disabled="empty" :loading="saving" @click="submit">
            {{ t('bulk.edit.submit', ids.length) }}
          </UiButton>
        </div>
        <div class="hint">{{ t('bulk.edit.runningNote') }}</div>
      </div>
    </template>
  </UiDialog>
</template>

<style scoped>
.fields {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.field-row {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.toggle {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-weight: 500;
  cursor: pointer;
}

.gateway {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.footer {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 8px;
  width: 100%;
}

.buttons {
  display: flex;
  gap: 10px;
}
</style>
