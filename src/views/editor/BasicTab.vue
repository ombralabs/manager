<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FieldError } from '../../api/types'
import SecretField from '../../components/SecretField.vue'
import AppIcon from '../../components/AppIcon.vue'
import TagInput from '../../components/TagInput.vue'
import UiField from '../../components/UiField.vue'
import UiSelect from '../../components/UiSelect.vue'
import TotpCode from './TotpCode.vue'
import { useFieldErrors } from '../../lib/errorText'
import type { EditorModel } from '../../lib/profileForm'
import { useGroupsStore } from '../../stores/groups'

const model = defineModel<EditorModel>({ required: true })
const props = defineProps<{ errors: FieldError[]; profileId: string | null }>()
const emit = defineEmits<{ openFingerprint: [] }>()
const { t } = useI18n()
const err = useFieldErrors(() => props.errors)
const groups = useGroupsStore()

/** Пустое значение — «Без группы»; id групп в выпадающем списке — строки. */
const groupOptions = computed(() => [
  { value: '', label: t('common.ungrouped') },
  ...groups.groups.map((group) => ({ value: String(group.id), label: group.name })),
])
const groupValue = computed({
  get: () => (model.value.group === null ? '' : String(model.value.group)),
  set: (value: string) => {
    model.value.group = value === '' ? null : Number(value)
  },
})
/** Код — у сохранённого профиля с ключом, который в форме не меняли и не удаляли. */
const showCode = computed(
  () => props.profileId !== null && model.value.totp.saved && model.value.totp.value === '' && !model.value.totp.clear,
)
const tagNames = computed(() => groups.tags.map((tag) => tag.name))
</script>

<template>
  <div class="stack">
    <section class="card section">
      <h3 class="section-title">{{ t('editor.workspace.profileFields') }}</h3>
      <div class="grid two">
        <UiField :label="t('editor.basic.name')" input-id="name" :error="err('name')">
          <input id="name" v-model="model.name" class="input" :class="{ invalid: err('name') }" maxlength="100" />
        </UiField>
        <UiField :label="t('editor.basic.group')" input-id="group" :error="err('group_id')">
          <UiSelect id="group" v-model="groupValue" :options="groupOptions" :invalid="!!err('group_id')" />
        </UiField>
      </div>
      <UiField :label="t('editor.basic.tags')" input-id="tags" :hint="t('editor.basic.tagsHint')" :error="err('tags')">
        <TagInput v-model="model.tags" :suggestions="tagNames" input-id="tags" :invalid="!!err('tags')" />
      </UiField>
      <UiField :label="t('editor.basic.note')" input-id="note">
        <textarea id="note" v-model="model.note" class="input" rows="3" />
      </UiField>
      <button type="button" class="browser-identity" @click="emit('openFingerprint')">
        <AppIcon name="fingerprint" :size="22" />
        <span><strong>{{ model.fingerprint.brand }} · macOS {{ model.fingerprint.os_version }}</strong><span>{{ t('editor.workspace.editFingerprint') }}</span></span>
        <AppIcon name="next" :size="16" />
      </button>
    </section>
    <section class="card section">
      <h3 class="section-title">{{ t('editor.basic.account') }}</h3>
      <UiField :label="t('editor.basic.platformDomain')" input-id="domain" :hint="t('editor.basic.platformDomainHint')">
        <input id="domain" v-model="model.platformDomain" class="input" spellcheck="false" />
      </UiField>
      <div class="grid two">
        <UiField :label="t('editor.basic.username')" input-id="username">
          <input id="username" v-model="model.username" class="input" autocomplete="off" spellcheck="false" />
        </UiField>
        <UiField :label="t('editor.basic.password')" input-id="password">
          <SecretField v-model="model.password" input-id="password" />
        </UiField>
      </div>
      <div class="grid two">
        <UiField :label="t('editor.basic.totp')" input-id="totp" :hint="t('editor.basic.totpHint')" :details="t('editor.basic.totpDetails')" :error="err('totp')">
          <SecretField v-model="model.totp" input-id="totp" kind="totp" :invalid="!!err('totp')" />
        </UiField>
        <UiField v-if="showCode || model.totp.value !== ''" :label="t('editor.basic.totpCode')">
          <TotpCode v-if="showCode" :key="profileId!" :profile-id="profileId!" />
          <span v-else-if="model.totp.value !== ''" class="hint">{{ t('editor.basic.totpUnsaved') }}</span>
        </UiField>
      </div>
      <div class="hint">{{ model.platformDomain.trim() ? t('editor.basic.autofillHint') : t('editor.basic.autofillNoDomain') }}</div>
    </section>
  </div>
</template>

<style scoped>
.stack {
  display: flex;
  flex-direction: column;
  gap: 16px;
}
.browser-identity { display: flex; align-items: center; gap: 12px; width: 100%; padding: 14px 16px; border: 1px solid var(--border); border-radius: var(--radius-control); background: var(--muted-bg); color: var(--accent-ink); text-align: left; cursor: pointer; }
.browser-identity > span { display: flex; flex: 1; flex-direction: column; gap: 4px; min-width: 0; }
.browser-identity strong { color: var(--text); font-size: 13px; font-weight: 500; }
.browser-identity span span { font-size: 12px; }
.browser-identity:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
</style>
