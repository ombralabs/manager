<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import type { FieldError, HumanizePreset, PersonaView, ProfileDetails } from '../../api/types'
import ConfirmDialog from '../../components/ConfirmDialog.vue'
import type { SelectOption } from '../../components/types'
import UiButton from '../../components/UiButton.vue'
import UiField from '../../components/UiField.vue'
import UiDetails from '../../components/UiDetails.vue'
import UiSegmented from '../../components/UiSegmented.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { useFieldErrors } from '../../lib/errorText'
import { notify } from '../../lib/notify'
import { type EditorModel, pasteKeyFromEvent } from '../../lib/profileForm'

const model = defineModel<EditorModel>({ required: true })
const props = defineProps<{ errors: FieldError[]; profileId: string | null; persona: PersonaView | null }>()
const emit = defineEmits<{ reset: [details: ProfileDetails] }>()
const { t, locale } = useI18n()
const err = useFieldErrors(() => props.errors)

const presetOptions = computed<SelectOption<HumanizePreset>[]>(() => [
  { value: 'default', label: t('editor.humanize.presetDefault') },
  { value: 'careful', label: t('editor.humanize.presetCareful') },
  { value: 'fast', label: t('editor.humanize.presetFast') },
])

/** Нажатие в поле горячей клавиши задаёт сочетание; Tab уходит дальше, Esc и одиночные клавиши — без изменений. */
function onPasteKey(event: KeyboardEvent) {
  if (event.key === 'Tab') return
  event.preventDefault()
  const key = pasteKeyFromEvent(event)
  if (key !== null) model.value.humanize.paste_key = key
}

const number = (value: number, digits = 0) =>
  new Intl.NumberFormat(locale.value, { maximumFractionDigits: digits, minimumFractionDigits: digits }).format(value)
/** Почерк строками: скорость набора — знаков в минуту, скорость мыши — от средней (множитель персоны — длительность). */
const personaRows = computed(() => {
  const p = props.persona
  if (!p) return []
  return [
    { label: t('editor.humanize.typing'), value: t('editor.humanize.typingValue', { n: number(p.keys_per_sec * 60) }) },
    { label: t('editor.humanize.typos'), value: t('editor.humanize.typosValue', { n: number(p.typo_rate * 100, 1) }) },
    { label: t('editor.humanize.dwell'), value: t('editor.humanize.dwellValue', { n: number(p.dwell_ms) }) },
    { label: t('editor.humanize.mouse'), value: t('editor.humanize.mouseValue', { n: number(100 / p.mouse_speed) }) },
    { label: t('editor.humanize.scroll'), value: t(`editor.humanize.scroll_${p.scroll_device}`) },
  ]
})

const resetOpen = ref(false)
const resetting = ref(false)
async function reset() {
  if (!props.profileId) return
  resetting.value = true
  try {
    emit('reset', await core.resetPersona(props.profileId))
    notify.info(t('editor.humanize.resetDone'))
  } catch (e) {
    notify.error(e)
  } finally {
    resetting.value = false
    resetOpen.value = false
  }
}
</script>

<template>
  <div class="stack">
    <section class="card section">
      <h2 class="section-title">{{ t('editor.humanize.title') }}</h2>
      <div class="hint">{{ t('editor.humanize.nextLaunch') }}</div>
      <div class="switch-row">
        <div class="intro">
          <div>{{ t('editor.humanize.enabled') }}</div>
          <div class="hint">{{ t('editor.humanize.enabledHint') }}</div>
          <UiDetails :text="t('editor.humanize.details')" />
        </div>
        <UiSwitch v-model="model.humanize.enabled" :label="t('editor.humanize.enabled')" />
      </div>
      <template v-if="model.humanize.enabled">
        <UiField :label="t('editor.humanize.preset')" :hint="t('editor.humanize.presetHint')">
          <UiSegmented v-model="model.humanize.preset" :options="presetOptions" :aria-label="t('editor.humanize.preset')" />
        </UiField>
        <div class="switch-row">
          <div class="intro">
            <div>{{ t('editor.humanize.blockInput') }}</div>
            <div class="hint">{{ t('editor.humanize.blockInputHint') }}</div>
          </div>
          <UiSwitch v-model="model.humanize.block_input" :label="t('editor.humanize.blockInput')" />
        </div>
        <UiField
          :label="t('editor.humanize.pasteKey')"
          input-id="paste-key"
          :hint="t('editor.humanize.pasteKeyHint')"
          :error="err('humanize.paste_key')"
        >
          <input
            id="paste-key"
            class="input mono"
            :class="{ invalid: err('humanize.paste_key') }"
            :value="model.humanize.paste_key"
            :placeholder="t('editor.humanize.pasteKeyNone')"
            readonly
            @keydown="onPasteKey"
          />
        </UiField>
      </template>
    </section>
    <section class="card section">
      <h2 class="section-title">{{ t('editor.humanize.persona') }}</h2>
      <div class="hint">{{ t('editor.humanize.personaHint') }}</div>
      <template v-if="persona">
        <dl class="persona">
          <template v-for="row in personaRows" :key="row.label">
            <dt>{{ row.label }}</dt>
            <dd>{{ row.value }}</dd>
          </template>
        </dl>
        <div>
          <UiButton :loading="resetting" @click="resetOpen = true">{{ t('editor.humanize.reset') }}</UiButton>
        </div>
      </template>
      <div v-else class="hint">{{ t('editor.humanize.personaNew') }}</div>
    </section>
    <ConfirmDialog
      v-model:open="resetOpen"
      :title="t('editor.humanize.resetTitle')"
      :description="t('editor.humanize.resetText')"
      :confirm-label="t('editor.humanize.reset')"
      :busy="resetting"
      danger
      @confirm="reset"
    />
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

.persona {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: 6px 24px;
  margin: 0;
}

.persona dt {
  color: var(--text-2);
}

.persona dd {
  margin: 0;
}
</style>
