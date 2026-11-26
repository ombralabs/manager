<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { toCoreError } from '../../api/errors'
import type { ExtCategory, FieldError } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import { codeText } from '../../lib/errorText'
import { setLabel } from '../../lib/extensions'
import { notify } from '../../lib/notify'
import { fold } from '../../lib/tags'

/** Пределы ядра (`store/extensions.rs`): название — как у групп. */
const NAME_MAX = 50
const REMARK_MAX = 500

/** Набор 0 скрыт под именем «Без набора»; хранится как «Общий» — оба и английское имя заняты. */
const RESERVED = new Set(['Общий', 'Без набора', 'No set'].map(fold))

const open = defineModel<boolean>('open', { required: true })
/** `null` — новый набор; иначе — переименование. */
const props = defineProps<{ category: ExtCategory | null }>()
const emit = defineEmits<{ saved: [category: ExtCategory] }>()
const { t } = useI18n()

const name = ref('')
const remark = ref('')
const errors = ref<FieldError[]>([])
const busy = ref(false)
const nameInput = ref<HTMLInputElement | null>(null)
const remarkInput = ref<HTMLTextAreaElement | null>(null)

watch(
  open,
  (value) => {
    if (!value) return
    name.value = props.category?.name ?? ''
    remark.value = props.category?.remark ?? ''
    errors.value = []
  },
  { immediate: true },
)

/** Кнопка на время запроса неактивна и теряет фокус — возвращаем его в поле с ошибкой. */
function focusFirstError() {
  const field = errors.value[0]?.field
  nextTick(() => (field === 'remark' ? remarkInput : nameInput).value?.focus())
}

/** Тексты ошибок свои у диалога: общие `validation.fields.name` относятся к имени профиля. */
function fieldText(field: 'name' | 'remark'): string | null {
  const code = errors.value.find((error) => error.field === field)?.code
  return code === undefined ? null : codeText(`extensions.setDialog.errors.${field}`, code, 'invalid')
}

const clear = (field: string) => (errors.value = errors.value.filter((error) => error.field !== field))

async function submit() {
  if (busy.value) return
  if (name.value.trim() === '') {
    errors.value = [{ field: 'name', code: 'required' }]
    focusFirstError()
    return
  }
  if (RESERVED.has(fold(name.value))) {
    errors.value = [{ field: 'name', code: 'reserved' }]
    focusFirstError()
    return
  }
  busy.value = true
  try {
    const saved = props.category
      ? await core.updateExtCategory(props.category.id, name.value, remark.value)
      : await core.createExtCategory(name.value, remark.value)
    open.value = false
    emit('saved', saved)
  } catch (e) {
    const err = toCoreError(e)
    // Своих полей два; ошибка другого поля или без поля — уведомлением, иначе она не была бы видна.
    const own = err.fields.filter((error) => error.field === 'name' || error.field === 'remark')
    if (own.length > 0) {
      errors.value = own
      focusFirstError()
    } else notify.error(err)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <UiDialog
    v-model:open="open"
    :title="category ? t('extensions.setDialog.editTitle', { name: setLabel(category) }) : t('extensions.setDialog.createTitle')"
    :width="480"
    :busy="busy"
  >
    <UiField :label="t('extensions.setDialog.name')" input-id="ext-set-name" :error="fieldText('name')">
      <input
        id="ext-set-name"
        ref="nameInput"
        v-model="name"
        class="input"
        :class="{ invalid: fieldText('name') }"
        :maxlength="NAME_MAX"
        :readonly="busy"
        @input="clear('name')"
        @keydown.enter="submit"
      />
    </UiField>
    <UiField :label="t('extensions.setDialog.remark')" input-id="ext-set-remark" :error="fieldText('remark')">
      <textarea
        id="ext-set-remark"
        ref="remarkInput"
        v-model="remark"
        class="input"
        :class="{ invalid: fieldText('remark') }"
        rows="3"
        :maxlength="REMARK_MAX"
        :readonly="busy"
        @input="clear('remark')"
      />
    </UiField>
    <template #footer>
      <UiButton :disabled="busy" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" :loading="busy" @click="submit">
        {{ category ? t('common.save') : t('extensions.setDialog.create') }}
      </UiButton>
    </template>
  </UiDialog>
</template>
