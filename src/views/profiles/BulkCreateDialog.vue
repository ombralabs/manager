<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { FieldError } from '../../api/types'
import type { SelectOption } from '../../components/types'
import TagInput from '../../components/TagInput.vue'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import UiSegmented from '../../components/UiSegmented.vue'
import UiSelect from '../../components/UiSelect.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { parseIntegerIn } from '../../lib/settings'
import { type BulkCreateDraft, BULK_CREATE_MAX, createDraftErrors, emptyCreateDraft, previewNames, toBulkCreateForm } from '../../lib/bulk'
import { useFieldErrors } from '../../lib/errorText'
import { fieldsOrToast, notify } from '../../lib/notify'
import { proxyOptions } from '../../lib/proxyOptions'
import { useGroupsStore } from '../../stores/groups'
import { useProfilesStore } from '../../stores/profiles'
import { useProxiesStore } from '../../stores/proxies'

const open = defineModel<boolean>('open', { required: true })
const { t } = useI18n()
const profiles = useProfilesStore()
const groups = useGroupsStore()

const draft = ref<BulkCreateDraft>(emptyCreateDraft())
const errors = ref<FieldError[]>([])
const proxies = useProxiesStore()
const saving = ref(false)
const err = useFieldErrors(() => errors.value)

/** Каждое открытие — чистая форма; группа — из фильтра списка, если он стоит на группе. */
watch(
  open,
  async (isOpen) => {
    if (!isOpen) return
    const group = profiles.filters.group
    draft.value = emptyCreateDraft(typeof group === 'number' ? group : null)
    errors.value = []
    try {
      await proxies.refresh()
    } catch (e) {
      notify.error(e)
    }
  },
  { immediate: true },
)

const count = computed(() => parseIntegerIn(draft.value.count, 1, BULK_CREATE_MAX) ?? 0)
const previewText = computed(() => {
  const names = count.value > 0 ? previewNames(draft.value.prefix.trim(), count.value) : null
  if (!names) return t('bulk.create.previewHint')
  return names.first === names.last ? names.first : t('bulk.create.preview', names)
})
const tagNames = computed(() => groups.tags.map((tag) => tag.name))

const groupOptions = computed(() => [
  { value: '', label: t('common.ungrouped') },
  ...groups.groups.map((group) => ({ value: String(group.id), label: group.name })),
])
const groupModel = computed({
  get: () => (draft.value.group === null ? '' : String(draft.value.group)),
  set: (value: string) => {
    draft.value.group = value === '' ? null : Number(value)
  },
})

const proxyModeOptions = computed<SelectOption<BulkCreateDraft['proxyMode']>[]>(() => [
  { value: 'random', label: t('bulk.create.proxyRandom') },
  { value: 'fixed', label: t('bulk.create.proxyFixed') },
])
const proxyChoices = computed(() => proxyOptions(proxies.rows, t('bulk.create.pickProxy'), t('bulk.create.proxyOff')))
const proxyModel = computed({
  get: () => (draft.value.proxyId === null ? '' : String(draft.value.proxyId)),
  set: (value: string) => {
    draft.value.proxyId = value === '' ? null : Number(value)
  },
})

/** Ошибки полей — у полей, ничего не создаётся; после создания список показывает новые профили. */
async function submit() {
  const local = createDraftErrors(draft.value)
  errors.value = local
  if (local.length > 0) return
  saving.value = true
  try {
    const created = await core.createProfilesBulk(toBulkCreateForm(draft.value))
    notify.info(t('bulk.create.done', created.length))
    open.value = false
    const group = draft.value.group
    if (group !== null) profiles.openGroup(group)
    // openGroup сам обновляет список; второй запрос list_profiles не нужен.
    await Promise.all([group !== null ? Promise.resolve() : profiles.refresh(), groups.refresh()])
  } catch (e) {
    fieldsOrToast(e, errors)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('bulk.create.title')" :width="560" :busy="saving">
    <div class="form">
      <div class="pair">
        <UiField :label="t('bulk.create.count')" input-id="bulk-create-count" :hint="t('bulk.create.countHint')" :error="err('count')">
          <input
            id="bulk-create-count"
            v-model="draft.count"
            class="input mono"
            :class="{ invalid: err('count') }"
            inputmode="numeric"
          />
        </UiField>
        <UiField :label="t('bulk.create.prefix')" input-id="bulk-create-prefix" :hint="previewText" :error="err('prefix')">
          <input
            id="bulk-create-prefix"
            v-model="draft.prefix"
            class="input"
            :class="{ invalid: err('prefix') }"
            maxlength="100"
          />
        </UiField>
      </div>
      <div class="grid two">
        <UiField :label="t('bulk.create.group')" input-id="bulk-create-group" :error="err('group_id')">
          <UiSelect id="bulk-create-group" v-model="groupModel" :options="groupOptions" :invalid="!!err('group_id')" />
        </UiField>
        <UiField :label="t('bulk.create.tags')" input-id="bulk-create-tags" :error="err('tags')">
          <TagInput v-model="draft.tags" input-id="bulk-create-tags" :suggestions="tagNames" :invalid="!!err('tags')" />
        </UiField>
      </div>
      <UiField :label="t('bulk.create.note')" input-id="bulk-create-note">
        <textarea id="bulk-create-note" v-model="draft.note" class="input" rows="2" />
      </UiField>
      <UiField
        :label="t('bulk.create.startUrls')"
        input-id="bulk-create-start-urls"
        :hint="t('bulk.create.startUrlsHint')"
        :error="err('start_urls')"
      >
        <textarea
          id="bulk-create-start-urls"
          v-model="draft.startUrls"
          class="input mono"
          :class="{ invalid: err('start_urls') }"
          rows="3"
          spellcheck="false"
        />
      </UiField>
      <div class="switch-row">
        <span>{{ t('bulk.create.restoreTabs') }}</span>
        <UiSwitch v-model="draft.restoreTabs" :label="t('bulk.create.restoreTabs')" />
      </div>
      <UiField
        :label="t('bulk.create.extraArgs')"
        input-id="bulk-create-extra-args"
        :hint="t('bulk.create.extraArgsHint')"
        :error="err('extra_args')"
      >
        <textarea
          id="bulk-create-extra-args"
          v-model="draft.extraArgs"
          class="input mono"
          :class="{ invalid: err('extra_args') }"
          rows="2"
          spellcheck="false"
        />
      </UiField>
      <UiField :label="t('bulk.create.gateway')" :error="err('proxy_id')">
        <div class="gateway">
          <UiSegmented v-model="draft.proxyMode" :options="proxyModeOptions" :aria-label="t('bulk.create.gateway')" />
          <template v-if="draft.proxyMode === 'fixed'">
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
      <div class="hint">{{ t('bulk.create.ownFingerprint') }}</div>
    </div>
    <template #footer>
      <UiButton :disabled="saving" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" :loading="saving" @click="submit">{{ t('bulk.create.submit', count) }}</UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.form {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.pair {
  display: grid;
  grid-template-columns: 120px minmax(0, 1fr);
  gap: 16px;
}

.switch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.gateway {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
</style>
