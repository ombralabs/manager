<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuItemIndicator, DropdownMenuPortal, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuRoot, DropdownMenuSeparator, DropdownMenuTrigger } from 'reka-ui'
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProfileLabel, ProfileRow } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import { notify } from '../../lib/notify'
import { labelStyle } from '../../lib/profileLabels'
import { useKeyboardInput } from '../../lib/useKeyboardInput'
import { useProfileLabelsStore } from '../../stores/profileLabels'
import ProfileLabelDialog from './ProfileLabelDialog.vue'

const props = defineProps<{ row: ProfileRow }>()
const { t } = useI18n()
const labels = useProfileLabelsStore()
const keyboardInput = useKeyboardInput()
const box = ref<HTMLElement | null>(null)
const ready = ref(false)
const open = ref(false)
const busy = ref(false)
const loading = ref(false)
const failed = ref(false)
const dialogOpen = ref(false)
const editing = ref<ProfileLabel | null>(null)
const selected = computed(() => props.row.label ? String(props.row.label.id) : 'none')
const caption = computed(() => props.row.label?.name ?? t('profiles.labels.none'))
const accessibleName = computed(() => t('profiles.labels.choose', { name: props.row.name, label: caption.value }))

// Как у RowMenu: закрытые строки не создают контекст Reka, чтобы список на 200 строк оставался быстрым.
async function prepare(how: 'hover' | 'focus' | 'open') {
  if (ready.value) return
  ready.value = true
  if (how === 'hover') return
  await nextTick()
  box.value?.querySelector('button')?.focus()
  if (how === 'open') open.value = true
}

async function load() {
  loading.value = true
  failed.value = false
  try { await labels.refresh() }
  catch (e) { failed.value = true; notify.error(e) }
  finally { loading.value = false }
}
watch(open, (value) => { if (value) void load() })

async function assign(labelId: number | null) {
  busy.value = true
  try { await labels.assign(props.row.id, labelId) }
  catch (e) { notify.error(e) }
  finally { busy.value = false }
}

function guardBusy(event: Event) {
  if (!busy.value || (event.type === 'keydown' && !['Enter', ' ', 'ArrowDown', 'ArrowUp'].includes((event as KeyboardEvent).key))) return
  event.preventDefault()
  event.stopImmediatePropagation()
}

function edit(label: ProfileLabel | null) {
  editing.value = label
  dialogOpen.value = true
}
</script>

<template>
  <span ref="box" class="profile-label-picker" :class="{ 'keyboard-focus': keyboardInput }">
    <button v-if="!ready" type="button" class="profile-label-trigger" :aria-label="accessibleName" :title="accessibleName" aria-haspopup="menu" aria-expanded="false"
      @pointerenter="prepare('hover')" @pointerdown.left="(e: PointerEvent) => { if (!e.ctrlKey) open = true }" @focus="prepare('focus')" @click="prepare('open')"
      @keydown.enter.prevent="prepare('open')" @keydown.space.prevent="prepare('open')" @keydown.down.prevent="prepare('open')">
      <span class="profile-label-chip" :class="{ none: !row.label }" :style="labelStyle(row.label)">{{ caption }}</span>
    </button>
    <DropdownMenuRoot v-else v-model:open="open">
      <DropdownMenuTrigger as-child>
        <button type="button" class="profile-label-trigger" :aria-disabled="busy" :aria-busy="busy" :aria-label="accessibleName" :title="accessibleName"
          @pointerdown.capture="guardBusy" @click.capture="guardBusy" @keydown.capture="guardBusy">
          <span class="profile-label-chip" :class="{ none: !row.label }" :style="labelStyle(row.label)">{{ caption }}</span>
          <AppIcon v-if="busy" name="spinner" :size="12" class="label-saving spin" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuPortal>
        <DropdownMenuContent align="start" :side-offset="6" class="popover profile-label-menu" :aria-label="t('profiles.labels.menu')">
          <DropdownMenuItem class="menu-item label-create" @select="edit(null)"><AppIcon name="plus" :size="16" />{{ t('profiles.labels.create') }}</DropdownMenuItem>
          <DropdownMenuSeparator class="label-separator" />
          <DropdownMenuRadioGroup :model-value="selected">
            <DropdownMenuRadioItem value="none" class="menu-item label-option" @select="assign(null)">
              <span class="profile-label-chip none">{{ t('profiles.labels.none') }}</span>
              <DropdownMenuItemIndicator><AppIcon name="check" :size="14" /></DropdownMenuItemIndicator>
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem v-for="label in labels.labels" :key="label.id" :value="String(label.id)" class="menu-item label-option" :title="label.name" @select="assign(label.id)">
              <span class="profile-label-chip" :style="labelStyle(label)">{{ label.name }}</span>
              <DropdownMenuItemIndicator><AppIcon name="check" :size="14" /></DropdownMenuItemIndicator>
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          <div v-if="loading && !labels.loaded" class="label-menu-note">{{ t('common.loading') }}</div>
          <DropdownMenuItem v-if="failed" class="menu-item" @select.prevent="load">{{ t('common.retry') }}</DropdownMenuItem>
          <template v-if="row.label && !row.label.preset">
            <DropdownMenuSeparator class="label-separator" />
            <DropdownMenuItem class="menu-item" @select="edit(row.label)"><AppIcon name="edit" :size="14" />{{ t('profiles.labels.edit') }}</DropdownMenuItem>
          </template>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>
    <ProfileLabelDialog v-if="dialogOpen" v-model:open="dialogOpen" :label="editing" @saved="(label) => { if (!editing) void assign(label.id) }" />
  </span>
</template>

<style>
.profile-label-chip { display: inline-block; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; border: 1px solid; border-radius: 4px; padding: 3px 8px; background: var(--surface); font-size: 11px; font-weight: 600; line-height: 18px; }
.profile-label-chip.none { border-color: var(--border-strong); color: var(--text-2); }
.profile-label-picker { display: inline-flex; max-width: 100%; min-width: 0; }
.profile-label-trigger { display: flex; align-items: center; gap: 5px; max-width: 100%; min-width: 0; min-height: 36px; padding: 3px 0; border: 0; outline: none; background: transparent; cursor: pointer; }
.profile-label-picker.keyboard-focus .profile-label-trigger:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; border-radius: 4px; }
.label-saving { flex-shrink: 0; color: var(--text-2); }
.profile-label-trigger[aria-disabled='true'] { opacity: 0.6; cursor: wait; }
.profile-label-menu { width: 224px; max-height: min(420px, var(--reka-dropdown-menu-content-available-height)); }
.profile-label-menu .label-option { justify-content: space-between; gap: 12px; min-height: 42px; }
.profile-label-menu .label-create { color: var(--accent-ink); font-size: 13px; }
.label-separator { height: 1px; margin: 4px 0; background: var(--border); }
.label-menu-note { padding: 8px 10px; color: var(--text-2); font-size: 12px; }
</style>
