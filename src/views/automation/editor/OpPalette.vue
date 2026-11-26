<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import AppIcon from '../../../components/AppIcon.vue'
import OpIcon from '../../../components/OpIcon.vue'
import type { OpInfo } from '../../../api/types'
import { opLabel } from '../../../lib/flowEdit'
import { PREF_OP_GROUPS_COLLAPSED, readPref, writePref } from '../../../lib/prefs'
import { useFlowEditorStore } from '../../../stores/flowEditor'
import type { PaletteDrag } from './paletteDrag'

const emit = defineEmits<{ add: [op: OpInfo]; drag: [drag: PaletteDrag | null]; drop: [drag: PaletteDrag] }>()

const ORDER = ['control', 'tabs', 'web', 'key', 'wait', 'data', 'files', 'proc', 'profile', 'ext']

const { t, te } = useI18n()
const store = useFlowEditorStore()
const search = ref('')
const paletteId = useId()
const searching = computed(() => search.value.trim() !== '')
const collapsed = ref(readPref<string[]>(
  PREF_OP_GROUPS_COLLAPSED,
  (raw) => Array.isArray(raw) ? raw.filter((id): id is string => typeof id === 'string' && ORDER.includes(id)) : null,
  [],
))
// Поиск раскрывает совпадения, сохраняя обычное состояние групп после очистки поля.
const searchCollapsed = ref<string[]>([])
watch(search, () => { searchCollapsed.value = [] })

const isCollapsed = (id: string) => (searching.value ? searchCollapsed.value : collapsed.value).includes(id)

function toggleGroup(id: string) {
  const target = searching.value ? searchCollapsed : collapsed
  target.value = target.value.includes(id) ? target.value.filter((other) => other !== id) : [...target.value, id]
  if (!searching.value) writePref(PREF_OP_GROUPS_COLLAPSED, target.value)
}

const label = (op: OpInfo) => opLabel(op.id, op.adspower, te, t)

const groups = computed(() => {
  const query = search.value.trim().toLowerCase()
  const ops = store.catalog.ops.filter(
    (op) => op.id !== 'start' && (!query || label(op).toLowerCase().includes(query)),
  )
  return ORDER.map((id) => ({ id, ops: ops.filter((op) => op.group === id) })).filter((g) => g.ops.length > 0)
})

// HTML5 DnD пересекается с нативным приёмом файлов Tauri. Захват указателя работает внутри WebView
// и не требует выключать импорт файлов перетаскиванием в остальных разделах приложения.
let pointer: { id: number; element: HTMLElement; op: OpInfo; x: number; y: number; dragging: boolean } | null = null
let suppressClick = false

function pointerDown(event: PointerEvent, op: OpInfo) {
  if (event.button !== 0 || pointer) return
  suppressClick = false
  const element = event.currentTarget as HTMLElement
  pointer = { id: event.pointerId, element, op, x: event.clientX, y: event.clientY, dragging: false }
  element.setPointerCapture?.(event.pointerId)
  window.addEventListener('pointermove', pointerMove)
  window.addEventListener('pointerup', pointerUp)
  window.addEventListener('pointercancel', pointerCancel)
  window.addEventListener('keydown', dragKey, true)
  window.addEventListener('blur', cancelDrag)
}

function pointerMove(event: PointerEvent) {
  if (!pointer || event.pointerId !== pointer.id) return
  if (!pointer.dragging && Math.hypot(event.clientX - pointer.x, event.clientY - pointer.y) < 5) return
  pointer.dragging = true
  suppressClick = true
  event.preventDefault()
  emit('drag', { op: pointer.op, x: event.clientX, y: event.clientY })
}

function releasePointer() {
  const previous = pointer
  pointer = null
  window.removeEventListener('pointermove', pointerMove)
  window.removeEventListener('pointerup', pointerUp)
  window.removeEventListener('pointercancel', pointerCancel)
  window.removeEventListener('keydown', dragKey, true)
  window.removeEventListener('blur', cancelDrag)
  if (previous?.element.hasPointerCapture?.(previous.id)) previous.element.releasePointerCapture(previous.id)
}

function pointerUp(event: PointerEvent) {
  if (!pointer || event.pointerId !== pointer.id) return
  const drop = pointer.dragging ? { op: pointer.op, x: event.clientX, y: event.clientY } : null
  releasePointer()
  if (drop) emit('drop', drop)
  emit('drag', null)
}

function cancelDrag() {
  if (!pointer) return
  suppressClick = true
  releasePointer()
  emit('drag', null)
}

function pointerCancel(event: PointerEvent) {
  if (event.pointerId === pointer?.id) cancelDrag()
}

function dragKey(event: KeyboardEvent) {
  if (event.key !== 'Escape') return
  event.preventDefault()
  event.stopPropagation()
  cancelDrag()
}

function clickOp(event: MouseEvent, op: OpInfo) {
  // После отпускания захваченного указателя браузер посылает click исходной кнопке.
  // Клавиатурная активация (detail === 0) остаётся доступной после отменённого переноса.
  if (pointer?.dragging || (suppressClick && event.detail !== 0)) return
  emit('add', op)
}

watch(() => store.path.join('/'), cancelDrag)
onBeforeUnmount(cancelDrag)
</script>

<template>
  <aside class="palette" :aria-label="t('automation.editor.paletteTitle')">
    <label class="search input">
      <AppIcon name="search" />
      <input v-model="search" type="search" :placeholder="t('automation.editor.paletteSearch')" :aria-label="t('automation.editor.paletteSearch')" />
    </label>
    <div class="groups">
      <section v-for="group in groups" :key="group.id" class="op-group" :data-group="group.id">
        <h3 class="group-heading">
          <button
            type="button"
            class="group-toggle"
            :aria-expanded="!isCollapsed(group.id)"
            :aria-controls="`${paletteId}-${group.id}`"
            @click="toggleGroup(group.id)"
          >
            <AppIcon name="chevron" :size="15" class="group-chevron" />
            <span class="group-title">{{ t(`automation.groups.${group.id}`) }}</span>
            <span class="group-count" aria-hidden="true">{{ group.ops.length }}</span>
          </button>
        </h3>
        <div :id="`${paletteId}-${group.id}`" :hidden="isCollapsed(group.id)" class="group-ops">
          <button v-for="op in group.ops" :key="op.id" type="button" class="op-item" :draggable="false"
            @click="clickOp($event, op)" @pointerdown="pointerDown($event, op)"
            @lostpointercapture="pointerCancel" @dragstart.prevent>
            <OpIcon :op="op.id" :size="16" class="item-icon" />
            <span class="name">{{ label(op) }}</span>
          </button>
        </div>
      </section>
      <div v-if="groups.length === 0" class="hint empty">{{ t('automation.editor.paletteEmpty') }}</div>
    </div>
  </aside>
</template>

<style scoped>
.palette { display: flex; flex-direction: column; gap: 8px; min-height: 0; width: 240px; }
.search { display: flex; flex-shrink: 0; align-items: center; gap: 8px; color: var(--text-2); cursor: text; }
.search:focus-within { border-color: var(--focus); box-shadow: 0 0 0 3px var(--focus-ring); }
.search input { flex-grow: 1; min-width: 0; border: 0; outline: 0; background: transparent; }
.groups { display: flex; flex-direction: column; gap: 6px; overflow: auto; min-height: 0; scrollbar-width: thin; }

.op-group {
  flex-shrink: 0;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-card);
  background: var(--surface);
}

.group-heading {
  margin: 0;
  font: inherit;
}

.group-toggle {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  min-height: 30px;
  padding: 5px 10px;
  border: 0;
  border-radius: calc(var(--radius-card) - 1px);
  background: var(--sidebar-active);
  color: var(--text);
  text-align: left;
  cursor: pointer;
}

.group-toggle[aria-expanded='true'] {
  border-bottom: 1px solid var(--border-strong);
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
}

.group-toggle:hover {
  color: var(--accent-ink);
}

.group-toggle:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: -2px;
}

.group-title {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  line-height: 18px;
  font-weight: 600;
}

.group-count {
  min-width: 22px;
  padding: 0 5px;
  border-radius: 5px;
  background: var(--surface);
  color: var(--accent-ink);
  font-size: 11px;
  line-height: 18px;
  text-align: center;
}

.group-chevron {
  color: var(--accent-ink);
}

.group-toggle[aria-expanded='false'] .group-chevron {
  transform: rotate(-90deg);
}

.group-ops {
  padding: 2px 4px;
}

.op-item { display: flex; align-items: flex-start; gap: 8px; width: 100%; padding: 4px 8px; border: 0; border-radius: 6px; background: transparent; text-align: left; cursor: grab; touch-action: none; user-select: none; }
.op-item:hover { background: var(--accent-soft); }
.item-icon { flex-shrink: 0; margin-top: 2px; color: var(--text-2); }
.op-item:hover .item-icon, .op-item:focus-visible .item-icon { color: var(--accent-ink); }
.op-item .name { flex: 1; min-width: 0; }
.op-item:focus-visible { outline: 2px solid var(--focus); }
.empty { padding: 8px 4px; }
</style>
