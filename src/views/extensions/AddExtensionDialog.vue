<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import { onFileDrop, pickExtensionFile } from '../../api/dialogs'
import AppIcon from '../../components/AppIcon.vue'
import UiButton from '../../components/UiButton.vue'
import UiCheckbox from '../../components/UiCheckbox.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import { baseName, storeId } from '../../lib/extensions'
import { notify } from '../../lib/notify'
import { useExtensionsStore } from '../../stores/extensions'

/** Холст «Расширения: добавление»: ссылка из магазина или файл; добавление идёт в фоне — карточка «Загружается». */
const open = defineModel<boolean>('open', { required: true })
const { t } = useI18n()
const store = useExtensionsStore()

const link = ref('')
/** Путь файла `.zip`/`.crx` или папки; ссылка и файл — что-то одно. */
const file = ref<string | null>(null)
const enable = ref(true)
const invalid = ref(false)
/** Файл тащат над окном — зона подсвечена. */
const over = ref(false)
const linkEl = ref<HTMLInputElement | null>(null)
const ready = computed(() => file.value !== null || link.value.trim() !== '')

/** Подписка на перетаскивание — только пока диалог открыт: на других экранах брошенный файл ничего не делает. */
let unlisten: (() => void) | null = null
async function listen() {
  try {
    const stop = await onFileDrop((event) => {
      over.value = event.type === 'enter' || event.type === 'over'
      if (event.type === 'drop' && event.paths[0] !== undefined) choose(event.paths[0])
    })
    if (open.value && unlisten === null) unlisten = stop
    else stop()
  } catch (e) {
    notify.error(e)
  }
}
function stopListening() {
  unlisten?.()
  unlisten = null
  over.value = false
}

watch(
  open,
  (value) => {
    if (!value) return stopListening()
    link.value = ''
    file.value = null
    enable.value = true
    invalid.value = false
    void listen()
  },
  { immediate: true },
)
onUnmounted(stopListening)

function choose(path: string) {
  file.value = path
  link.value = ''
  invalid.value = false
}

async function pick() {
  try {
    const path = await pickExtensionFile()
    if (path !== null) choose(path)
  } catch (e) {
    notify.error(e)
  }
}

function onInput() {
  invalid.value = false
  if (link.value !== '') file.value = null
}

/** «Открыть магазин»: текст поля — запрос поиска; пусто — главная магазина. */
function openStore() {
  core.openStoreSearch(link.value.trim()).catch(notify.error)
}

/** Ссылку проверяем здесь (правило `store_id` ядра): диалог закрывается сразу, ошибки ядра придут уведомлением. */
function submit() {
  if (!ready.value) return
  if (file.value !== null) {
    void store.add({ file: file.value }, enable.value)
  } else {
    const id = storeId(link.value)
    if (id === null) {
      invalid.value = true
      void nextTick(() => linkEl.value?.focus())
      return
    }
    void store.add({ store: id }, enable.value)
  }
  open.value = false
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t('extensions.addExtension')" :width="560">
    <UiField
      :label="t('extensions.addDialog.link')"
      input-id="ext-url"
      :hint="t('extensions.addDialog.linkHint')"
      :error="invalid ? t('extensions.addDialog.invalid') : null"
    >
      <template #badge>
        <button type="button" class="link store" @click="openStore">
          {{ t('extensions.addDialog.openStore') }}<AppIcon name="external" :size="16" />
        </button>
      </template>
      <input
        id="ext-url"
        ref="linkEl"
        v-model="link"
        class="input mono"
        :class="{ invalid }"
        type="url"
        spellcheck="false"
        autocomplete="off"
        placeholder="https://chromewebstore.google.com/detail/…"
        :aria-invalid="invalid || undefined"
        @input="onInput"
        @keydown.enter="submit"
      />
    </UiField>
    <div class="or" aria-hidden="true">{{ t('extensions.addDialog.or') }}</div>
    <div class="file">
      <div id="ext-file-label" class="label">{{ t('extensions.addDialog.file') }}</div>
      <div class="drop" :class="{ over }" role="group" aria-labelledby="ext-file-label">
        <template v-if="file === null">
          <div class="drop-text"><AppIcon name="upload" />{{ t('extensions.addDialog.drop') }}</div>
          <UiButton @click="pick">{{ t('extensions.addDialog.pick') }}</UiButton>
        </template>
        <template v-else>
          <div class="drop-text mono" :title="file">{{ baseName(file) }}</div>
          <UiButton variant="ghost" @click="file = null">{{ t('extensions.addDialog.clear') }}</UiButton>
        </template>
      </div>
    </div>
    <label class="enable">
      <UiCheckbox v-model="enable" :ariaLabel="t('extensions.addDialog.enable')" />
      <span aria-hidden="true">{{ t('extensions.addDialog.enable') }}</span>
    </label>
    <template #footer>
      <UiButton @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="primary" :disabled="!ready" @click="submit">{{ t('extensions.addDialog.submit') }}</UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
/* Размеры и отступы — артборд «Расширения: добавление». */
.store {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  line-height: 18px;
  font-weight: 600;
}

.or {
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 13px;
  line-height: 18px;
  color: var(--text-2);
}

.or::before,
.or::after {
  content: '';
  flex-grow: 1;
  height: 1px;
  background: var(--border);
}

.file {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.label {
  font-size: 13px;
  line-height: 18px;
  color: var(--text-2);
}

.drop {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  height: 120px;
  border: 1.5px dashed var(--border-strong);
  border-radius: 12px;
  background: var(--muted-bg);
}

.drop.over {
  border-color: var(--accent);
  background: var(--accent-soft);
}

.drop-text {
  display: flex;
  align-items: center;
  gap: 8px;
  max-width: 100%;
  padding: 0 16px;
  overflow: hidden;
  font-size: 13px;
  line-height: 18px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.drop-text .icon {
  flex-shrink: 0;
  color: var(--text-2);
}

.enable {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 24px;
  cursor: pointer;
}
</style>
