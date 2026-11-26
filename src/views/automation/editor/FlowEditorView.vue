<script setup lang="ts">
import { useVueFlow } from '@vue-flow/core'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave, onBeforeRouteUpdate, useRoute, useRouter, type RouteLocationNormalized } from 'vue-router'
import { core } from '../../../api/core'
import { pickFlowFile, pickSaveFlow } from '../../../api/dialogs'
import type { FlowDoc, OpInfo } from '../../../api/types'
import ConfirmDialog from '../../../components/ConfirmDialog.vue'
import { addNode, align, levelOf, nodeHeight, nodeWidth, NODE_W, nodeName, pathTo } from '../../../lib/flowEdit'
import { notify } from '../../../lib/notify'
import { useFlowDebugStore } from '../../../stores/flowDebug'
import { useFlowEditorStore } from '../../../stores/flowEditor'
import { useAutomationStore } from '../../../stores/automation'
import TaskDialog from '../TaskDialog.vue'
import DebugPanel from './DebugPanel.vue'
import EditorToolbar from './EditorToolbar.vue'
import FlowCanvas from './FlowCanvas.vue'
import FlowSettingsDialog from './FlowSettingsDialog.vue'
import NodeSettingsPanel from './NodeSettingsPanel.vue'
import OpPalette from './OpPalette.vue'
import type { PaletteDrag } from './paletteDrag'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useFlowEditorStore()
const debug = useFlowDebugStore()
const automation = useAutomationStore()
const { screenToFlowCoordinate } = useVueFlow()

const loaded = ref(false)
const paletteDrag = ref<PaletteDrag | null>(null)
const canvas = ref<InstanceType<typeof FlowCanvas> | null>(null)
function dropOperation(drag: PaletteDrag) {
  canvas.value?.dropOperation(drag)
  paletteDrag.value = null
}
const saving = ref(false)
const importing = ref(false)
const runOpen = ref(false)
const settingsOpen = ref(false)
const ops = computed(() => new Map(store.catalog.ops.map((op) => [op.id, op])))

const isEditor = (path: string) => path.startsWith('/automation/flows/')
/** Поле ввода: у него свой ⌘Z, и его черновик отдаётся на blur. */
const isField = (el: HTMLElement) => el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable
const idOf = (to: RouteLocationNormalized) => (to.params.id ? Number(to.params.id) : null)

let alive = true
async function open() {
  debug.reset()
  runOpen.value = false
  loaded.value = false
  try {
    if (await store.load(idOf(route))) loaded.value = true
  } catch (e) {
    // С экрана ушли, пока шла загрузка: её ошибка не к новому экрану, и уводить с него нельзя.
    if (!alive) return
    notify.error(e)
    await router.replace('/automation/flows')
  }
}
onMounted(open)
// Уход с экрана останавливает идущую отладку: окно профиля без редактора никто не остановит.
onBeforeUnmount(() => {
  alive = false
  debug.reset()
})
// Экран живёт между адресами редактора (`viewKey` в router.ts): другой процесс или «Создать» загружаются здесь;
// адрес нового процесса после первого сохранения ведёт на уже открытый документ — его правки и история остаются.
watch(() => route.params.id, () => { if (isEditor(route.path) && idOf(route) !== store.id) void open() })

// Уход с правками: переход отменяется и открывается вопрос, «Уйти» повторяет переход. Пока вопрос открыт, новые
// переходы тоже отменяются — вопрос остаётся о первом. Ждать ответа в охраннике нельзя: роутер считает любой новый
// переход отменой ожидающего, и «Уйти» уже никуда бы не вело. Пока процесс не загружен, правок этого экрана нет.
const leaveOpen = ref(false)
let leaveTo = ''
/** Шаг по истории («Назад», «Вперёд») повторяется тем же шагом: новая запись отрезала бы путь «Вперёд». */
let leaveDelta = 0
/** Повтор подтверждённого ухода: следующий переход пропускается без вопроса. */
let leaving = false
let pop: { to: string; delta: number } | null = null
onBeforeUnmount(router.options.history.listen((to, _from, info) => { pop = { to, delta: info.delta } }))
function guard(to: RouteLocationNormalized) {
  const delta = pop?.to === to.fullPath ? pop.delta : 0
  pop = null
  if (leaving) {
    leaving = false
    return true
  }
  if (!loaded.value || !store.dirty || (isEditor(to.path) && idOf(to) === store.id)) return true
  if (!leaveOpen.value) {
    leaveTo = to.fullPath
    leaveDelta = delta
    leaveOpen.value = true
  }
  return false
}
onBeforeRouteLeave(guard)
onBeforeRouteUpdate(guard)
function answer(go: boolean) {
  leaveOpen.value = false
  if (!go) return
  leaving = true
  if (leaveDelta !== 0) router.go(leaveDelta)
  else void router.push(leaveTo)
}
const leaveModel = computed({ get: () => leaveOpen.value, set: (v: boolean) => { if (!v) answer(false) } })

async function save() {
  // Поле в фокусе отдаёт черновик на blur («1.» → 1, «007» → 7): иначе в процесс ушла бы промежуточная строка.
  // Кнопка фокус не забирает (WebKit), так что поле может быть в фокусе и при щелчке по «Сохранить»; кнопку же,
  // нажатую с клавиатуры, фокус не покидает.
  const active = document.activeElement
  if (active instanceof HTMLElement && isField(active)) active.blur()
  saving.value = true
  try {
    const wasNew = store.id === null
    const id = await store.save()
    // `null` — записан прежний процесс, а открыт уже другой: «Процесс сохранён» читалось бы про него.
    if (id === null) return
    notify.info(t('automation.editor.saved'))
    // Пока шла запись, могли уйти с экрана — тогда адрес не трогаем. Открытому вопросу об уходе смена адреса
    // не мешает: ожидающего перехода нет, «Уйти» повторит его сам.
    if (wasNew && route.path === '/automation/flows/new') await router.replace(`/automation/flows/${id}`)
  } catch (e) {
    notify.error(e)
  } finally {
    saving.value = false
  }
}

async function exportFlow() {
  if (store.id === null || !store.doc) return
  try {
    const path = await pickSaveFlow(`${store.doc.name}.json`)
    if (!path) return
    await core.exportFlowFile(store.id, path)
    notify.info(t('automation.editor.exported'))
  } catch (e) {
    notify.error(e)
  }
}

async function importFlow() {
  if (importing.value) return
  importing.value = true
  const origin = route.fullPath
  try {
    const path = await pickFlowFile()
    if (!path || !alive) return
    const imported = await automation.importFlow(path)
    notify.info(t('automation.flows.imported', { name: imported.name }))
    if (alive && route.fullPath === origin) await router.push(`/automation/flows/${imported.id}`)
  } catch (e) {
    if (alive) notify.error(e)
  } finally {
    importing.value = false
  }
}

function firstError() {
  const first = store.report.errors[0]
  if (!first || !store.doc) return
  // Ошибки без узла (имя, переменные, цикл) показаны в панели Start.
  const id = first.node ?? store.doc.nodes.find((n) => n.op === 'start')?.id
  const path = id ? pathTo(store.doc.nodes, id) : null
  if (!id || !path) return
  store.path = path
  store.openNodeSettings(id)
}

function add(op: OpInfo) {
  if (!store.doc) return
  const level = levelOf(store.doc, store.path)
  const prev = level.nodes.find((n) => n.id === store.selected)
  let pos: [number, number]
  if (prev) {
    pos = [(prev.pos?.[0] ?? 0) + nodeWidth(prev) + 40, prev.pos?.[1] ?? 0]
  } else {
    const box = document.querySelector('.flow-editor .canvas')?.getBoundingClientRect()
    const c = box ? screenToFlowCoordinate({ x: box.left + box.width / 2, y: box.top + box.height / 2 }) : { x: 0, y: 0 }
    pos = [Math.round(c.x - NODE_W / 2), Math.round(c.y - nodeHeight({ id: '', op: op.id }, op) / 2)]
  }
  store.apply((doc) => {
    const added = addNode(doc, store.path, op, pos, prev?.id, ops.value)
    store.selected = added.id
    return added.doc
  })
}

const groupTitles = computed(() => {
  const titles: string[] = []
  let nodes = store.doc?.nodes ?? []
  for (const id of store.path) {
    const group = nodes.find((n) => n.id === id)
    const name = group ? nodeName(group) : ''
    titles.push(name ? t('automation.editor.breadcrumbGroup', { name }) : t('automation.ops.control_group'))
    nodes = group?.nodes ?? []
  }
  return titles
})

function onKey(e: KeyboardEvent) {
  // Под диалогом отмена правила бы процесс за его спиной.
  if (settingsOpen.value || leaveOpen.value || runOpen.value || importing.value || paletteDrag.value) return
  const key = e.key.toLowerCase()
  if (!(e.metaKey || e.ctrlKey) || !['z', 'c', 'v', 'g', 'a'].includes(key) || (e.target instanceof HTMLElement && isField(e.target))) return
  // Выделенный текст страницы (например, в журнале отладки) копирует браузер.
  if (key === 'c' && window.getSelection()?.toString()) return
  e.preventDefault()
  if (key === 'a') canvas.value?.selectAll()
  else if (key === 'c') store.copy()
  else if (key === 'v') store.paste()
  else if (key === 'g') {
    const only = store.selection.length === 1 ? store.selection[0] : undefined
    if (!e.shiftKey) store.group()
    else if (only && store.doc && levelOf(store.doc, store.path).nodes.find((n) => n.id === only)?.op === 'control.group') store.ungroup(only)
  }
  else if (e.shiftKey) store.redo()
  else store.undo()
}
onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
const apply = (change: (doc: FlowDoc) => FlowDoc) => store.apply(change)
</script>

<template>
  <main :class="['page flow-editor', { placing: paletteDrag }]">
    <template v-if="loaded && store.doc">
      <EditorToolbar :saving="saving" :importing="importing" @back="router.push('/automation/flows')" @save="save" @export="exportFlow" @import="importFlow" @run="runOpen = true"
        @align="store.apply((doc) => align(doc, store.path, ops))" @settings="settingsOpen = true" @errors="firstError" />
      <nav v-if="store.path.length > 0" class="crumbs" :aria-label="t('automation.editor.breadcrumbs')">
        <button type="button" class="link" @click="store.leaveTo(0)">{{ t('automation.editor.breadcrumbFlow') }}</button>
        <template v-for="(title, i) in groupTitles" :key="i">
          <span class="sep">›</span>
          <button v-if="i < groupTitles.length - 1" type="button" class="link" @click="store.leaveTo(i + 1)">{{ title }}</button>
          <span v-else class="current">{{ title }}</span>
        </template>
      </nav>
      <div class="body">
        <OpPalette @add="add" @drag="paletteDrag = $event" @drop="dropOperation" />
        <div class="center">
          <div class="canvas-workspace">
            <FlowCanvas ref="canvas" :palette-drag="paletteDrag" />
            <NodeSettingsPanel />
          </div>
          <DebugPanel />
        </div>
      </div>
      <FlowSettingsDialog v-model:open="settingsOpen" :doc="store.doc" @apply="apply" />
      <TaskDialog v-model:open="runOpen" :flow-id="store.id" :profile-ids="null" @created="router.push('/automation/tasks')" />
    </template>
    <ConfirmDialog v-model:open="leaveModel" :title="t('automation.editor.leaveTitle')" :description="t('automation.editor.leaveText')"
      :confirm-label="t('automation.editor.leave')" danger @confirm="answer(true)" />
  </main>
</template>

<style scoped>
.flow-editor { gap: 12px; padding: 16px 24px; overflow: hidden; }
.flow-editor.placing, .flow-editor.placing :deep(*) { cursor: grabbing !important; }
.body { display: flex; flex-grow: 1; min-height: 0; gap: 12px; }
.body :deep(.canvas) { border: 1px solid var(--border); border-radius: var(--radius-card); overflow: hidden; }
.center { display: flex; flex-direction: column; flex-grow: 1; min-width: 0; }
.canvas-workspace { position: relative; display: flex; flex-grow: 1; min-height: 0; }
.canvas-workspace :deep(.canvas) { isolation: isolate; }
.crumbs { display: flex; align-items: center; gap: 8px; flex-shrink: 0; font-size: 13px; }
.sep { color: var(--text-2); }
.current { font-weight: 500; }
</style>
