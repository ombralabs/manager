<script setup lang="ts">
import { PopoverAnchor, PopoverContent, PopoverPortal, PopoverRoot } from 'reka-ui'
import { computed, nextTick, ref, useId } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProfileRow } from '../../api/types'
import { toCoreError } from '../../api/errors'
import AppIcon from '../../components/AppIcon.vue'
import UiButton from '../../components/UiButton.vue'
import UiCheckbox from '../../components/UiCheckbox.vue'
import { notify } from '../../lib/notify'
import { fold, mergeTags, sameTag } from '../../lib/tags'
import { useKeyboardInput } from '../../lib/useKeyboardInput'
import { useGroupsStore } from '../../stores/groups'

const props = defineProps<{ row: ProfileRow }>()
const { t } = useI18n()
const groups = useGroupsStore()
const keyboardInput = useKeyboardInput()
const trigger = ref<HTMLButtonElement | null>(null)
const search = ref<HTMLInputElement | null>(null)
const contentId = useId()
const open = ref(false)
const query = ref('')
const error = ref('')
const busy = ref(false)
const known = ref<string[]>([])
let outside = false
const selected = (tag: string) => props.row.tags.some((name) => sameTag(name, tag))
const uniqueNames = (names: string[]) => [...new Map(names.map((name) => [fold(name), name])).values()]
const catalog = computed(() => uniqueNames([...known.value, ...groups.tags.filter((tag) => tag.profile_count > 0).map((tag) => tag.name), ...props.row.tags])
  .sort((a, b) => fold(a).localeCompare(fold(b))))
const choices = computed(() => catalog.value.filter((tag) => fold(tag).includes(fold(query.value))))
const additions = computed(() => mergeTags([], query.value).filter((tag) => !selected(tag)))
const showAdd = computed(() => additions.value.length > 0 && !(additions.value.length === 1 && catalog.value.some((tag) => sameTag(tag, query.value))))
const accessibleName = computed(() => t('profiles.quickTags.edit', { name: props.row.name, tags: props.row.tags.join(', ') || t('profiles.quickTags.empty') }))

function onOpenFocus(event: Event) {
  event.preventDefault()
  outside = false
  known.value = uniqueNames([...groups.tags.filter((tag) => tag.profile_count > 0).map((tag) => tag.name), ...props.row.tags])
  query.value = error.value = ''
  search.value?.focus({ preventScroll: true })
}
function onOutside(event: CustomEvent<{ originalEvent: Event }>) {
  const target = event.detail.originalEvent.target as Node | null
  if (target && trigger.value?.contains(target)) event.preventDefault()
  else outside = true
}
function onCloseFocus(event: Event) {
  event.preventDefault()
  if (!outside) trigger.value?.focus({ preventScroll: true })
}
function guardBusy(event: Event) {
  if (!busy.value || (event.type === 'keydown' && !['Enter', ' '].includes((event as KeyboardEvent).key))) return
  event.preventDefault()
  event.stopImmediatePropagation()
}
async function change(add: string[], remove: string[], clearQuery = false) {
  if (busy.value || (!add.length && !remove.length)) return
  if (add.some((tag) => [...tag].length > 30)) { error.value = t('validation.fields.tags.invalid'); return }
  busy.value = true
  error.value = ''
  try {
    const saved = await groups.changeTags(props.row.id, add, remove)
    known.value = uniqueNames([...known.value, ...saved])
    if (clearQuery) query.value = ''
    await nextTick()
    if (open.value && document.activeElement === document.body) search.value?.focus({ preventScroll: true })
  } catch (e) {
    if (toCoreError(e).code === 'Validation') error.value = t('validation.fields.tags.invalid')
    else notify.error(e)
  } finally { busy.value = false }
}
</script>

<template>
  <span :class="['profile-tags-picker', { 'keyboard-focus': keyboardInput }]">
    <button ref="trigger" type="button" class="tags-trigger" :aria-label="accessibleName" :title="accessibleName" aria-haspopup="dialog" :aria-expanded="open" :aria-controls="open ? contentId : undefined"
      @click="open = !open" @keydown.down.prevent="open = true">
      <span v-for="tag in row.tags.slice(0, 2)" :key="tag" class="profile-tag">{{ tag }}</span>
      <span v-if="row.tags.length > 2" class="tags-more">+{{ row.tags.length - 2 }}</span>
      <span v-if="!row.tags.length" class="tags-empty"><AppIcon name="plus" :size="12" />{{ t('profiles.quickTags.add') }}</span>
    </button>
    <!-- Контекст всплывающего окна создаётся только у открытой ячейки. -->
    <PopoverRoot v-if="open" :open="open" @update:open="open = $event">
      <PopoverAnchor :reference="trigger ?? undefined" as="span" class="tags-anchor" />
      <PopoverPortal>
        <PopoverContent :id="contentId" align="start" :side-offset="6" :collision-padding="12" class="popover profile-tags-popover" role="dialog" :aria-label="t('profiles.quickTags.title', { name: row.name })"
          @open-auto-focus="onOpenFocus" @close-auto-focus="onCloseFocus" @interact-outside="onOutside">
          <header class="tags-head"><span>{{ t('columns.tags') }}<span class="tags-count">{{ row.tags.length }}</span></span><UiButton size="icon" variant="ghost" icon="close" :icon-size="14" :aria-label="t('common.close')" @click="open = false" /></header>
          <div :aria-busy="busy" @pointerdown.capture="guardBusy" @click.capture="guardBusy" @keydown.capture="guardBusy">
            <label class="tags-search input"><AppIcon name="search" :size="15" /><input ref="search" v-model="query" type="text" :readonly="busy" :placeholder="t('profiles.quickTags.search')" :aria-label="t('profiles.quickTags.search')" :aria-invalid="!!error" :aria-describedby="error ? `${contentId}-error` : undefined" autocomplete="off" spellcheck="false" @input="error = ''" @keydown.enter.prevent="change(additions, [], true)" /></label>
            <div v-if="error" :id="`${contentId}-error`" class="error-text tags-error" role="alert">{{ error }}</div>
            <button v-if="showAdd" type="button" class="tags-create" :aria-disabled="busy" @click="change(additions, [], true)"><AppIcon name="plus" :size="14" /><span>{{ additions.length === 1 ? t('profiles.quickTags.addNamed', { tag: additions[0] }) : t('profiles.quickTags.addMany', { n: additions.length }) }}</span></button>
            <div class="tags-choices" role="group" :aria-label="t('profiles.quickTags.choose')">
              <label v-for="tag in choices" :key="fold(tag)" class="tag-choice" :title="tag"><UiCheckbox :model-value="selected(tag)" :ariaLabel="tag" :aria-disabled="busy" @update:model-value="(checked) => change(checked ? [tag] : [], checked ? [] : [tag])" /><span class="profile-tag">{{ tag }}</span></label>
              <p v-if="!choices.length && !showAdd" class="tags-note">{{ t('profiles.quickTags.noTags') }}</p>
            </div>
          </div>
          <footer class="tags-note"><AppIcon v-if="busy" name="spinner" :size="12" class="spin" />{{ t(busy ? 'profiles.quickTags.saving' : 'profiles.quickTags.hint') }}</footer>
        </PopoverContent>
      </PopoverPortal>
    </PopoverRoot>
  </span>
</template>

<style>
.profile-tag { display: inline-block; min-width: 0; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; height: 18px; padding: 0 5px; border-radius: 4px; background: var(--muted-bg); color: var(--text-2); font-size: 10px; font-weight: 400; line-height: 18px; }
.profile-tags-picker { display: block; min-width: 0; }
.tags-trigger { display: flex; align-items: center; gap: 3px; width: 100%; min-height: 32px; padding: 3px 0; border: 0; outline: none; background: transparent; text-align: left; cursor: pointer; }
.profile-tags-picker.keyboard-focus .tags-trigger:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; border-radius: 4px; }
.tags-trigger .profile-tag { flex: 0 1 auto; }
.tags-more { flex-shrink: 0; font-size: 10px; color: var(--text-2); }
.tags-empty { display: inline-flex; align-items: center; gap: 3px; color: var(--text-2); font-size: 11px; }
.tags-trigger:hover .tags-empty { color: var(--accent-ink); }
.tags-anchor { display: none; }
.profile-tags-popover { width: 288px; max-width: calc(100vw - 24px); max-height: min(420px, var(--reka-popover-content-available-height, 420px)); padding: 10px; }
.tags-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding-bottom: 8px; font-size: 13px; font-weight: 500; }
.tags-head .btn.square { width: 24px; height: 24px; }
.tags-count { margin-left: 7px; color: var(--text-2); font-size: 11px; font-weight: 400; font-variant-numeric: tabular-nums; }
.tags-search { display: flex; align-items: center; gap: 7px; height: 36px; padding: 0 9px; }
.tags-search:focus-within { border-color: var(--focus); }
.tags-search > .icon { flex-shrink: 0; color: var(--text-2); }
.tags-search input { min-width: 0; width: 100%; border: 0; outline: 0; background: transparent; font: inherit; font-size: 12px; }
.tags-choices { display: flex; flex-direction: column; max-height: 240px; overflow-y: auto; margin-top: 6px; }
.tag-choice { display: flex; align-items: center; gap: 9px; min-height: 34px; padding: 5px 7px; border-radius: 4px; cursor: pointer; }
.tag-choice:hover { background: var(--accent-soft); }
.tags-create { display: flex; align-items: center; gap: 7px; width: 100%; min-height: 34px; padding: 7px; border: 0; border-radius: 4px; background: transparent; color: var(--accent-ink); font: inherit; font-size: 12px; text-align: left; cursor: pointer; }
.tags-create span { min-width: 0; overflow-wrap: anywhere; }
.tags-create:hover { background: var(--accent-soft); }
.tags-create:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
.tags-note { display: flex; align-items: center; gap: 5px; margin: 0; padding: 8px 3px 0; color: var(--text-2); font-size: 11px; line-height: 16px; }
.tags-error { margin-top: 6px; }
</style>
