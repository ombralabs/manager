<script setup lang="ts">
import { nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import type { Bookmark } from '../../api/types'
import UiButton from '../../components/UiButton.vue'
import { BOOKMARKS_MAX, bookmarkRowError } from '../../lib/profileForm'

const list = defineModel<Bookmark[]>({ required: true })
defineProps<{ error: string | null }>()
const { t } = useI18n()

async function add() {
  list.value = [...list.value, { title: '', url: '' }]
  await nextTick()
  document.getElementById(`bookmark-title-${list.value.length - 1}`)?.focus()
}

function remove(index: number) {
  list.value = list.value.filter((_, i) => i !== index)
}

function update(index: number, field: keyof Bookmark, value: string) {
  list.value = list.value.map((b, i) => (i === index ? { ...b, [field]: value } : b))
}

const rowError = (b: Bookmark) => {
  const field = bookmarkRowError(b)
  if (field === 'title') return t('editor.launch.bookmarkTitleError')
  if (field === 'url') return t('editor.launch.bookmarkUrlError')
  return null
}
</script>

<template>
  <div class="bookmarks">
    <div v-if="error" class="notice">{{ error }}</div>
    <div v-for="(bookmark, index) in list" :key="index" class="row">
      <input
        :id="`bookmark-title-${index}`"
        class="input"
        :class="{ invalid: bookmarkRowError(bookmark) === 'title' }"
        :value="bookmark.title"
        :placeholder="t('editor.launch.bookmarkTitle')"
        :aria-label="t('editor.launch.bookmarkTitle')"
        @input="update(index, 'title', ($event.target as HTMLInputElement).value)"
      />
      <input
        class="input mono"
        :class="{ invalid: bookmarkRowError(bookmark) === 'url' }"
        :value="bookmark.url"
        placeholder="https://"
        :aria-label="t('editor.launch.bookmarkUrl')"
        spellcheck="false"
        @input="update(index, 'url', ($event.target as HTMLInputElement).value)"
      />
      <UiButton
        size="icon"
        variant="ghost"
        icon="close"
        :aria-label="t('editor.launch.removeBookmark', { title: bookmark.title })"
        @click="remove(index)"
      />
      <div v-if="rowError(bookmark)" class="row-error">{{ rowError(bookmark) }}</div>
    </div>
    <UiButton icon="plus" :disabled="list.length >= BOOKMARKS_MAX" @click="add">{{ t('editor.launch.addBookmark') }}</UiButton>
  </div>
</template>

<style scoped>
.bookmarks {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 8px;
}

.row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 2fr) 32px;
  gap: 8px;
  width: 100%;
}

.row-error {
  grid-column: 1 / -1;
  color: var(--danger-text);
  font-size: 12px;
}
</style>
