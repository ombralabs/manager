<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { pickTableFile } from '../../api/dialogs'
import { core } from '../../api/core'
import { toCoreError } from '../../api/errors'
import type { FieldError, TablePreview, TaskDetails, TaskForm, VarDecl } from '../../api/types'
import ProfileStatusFilter from '../../components/ProfileStatusFilter.vue'
import BrowserModeField from '../../components/BrowserModeField.vue'
import TagInput from '../../components/TagInput.vue'
import UiButton from '../../components/UiButton.vue'
import UiDialog from '../../components/UiDialog.vue'
import UiField from '../../components/UiField.vue'
import UiSegmented from '../../components/UiSegmented.vue'
import UiSelect from '../../components/UiSelect.vue'
import { useFieldErrors } from '../../lib/errorText'
import { fieldsOrToast, notify } from '../../lib/notify'
import { formDefault, parseStrictNumber } from '../../lib/paramDefault'
import { defaultProfileCriteria, filterProfiles } from '../../lib/profileFilter'
import { useGroupsStore } from '../../stores/groups'
import { useAutomationStore } from '../../stores/automation'
import ParamField from './ParamField.vue'

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{ flowId: number | null; profileIds: string[] | null; taskId?: number | null }>()
const emit = defineEmits<{ created: [taskId: number]; updated: [taskId: number] }>()
const { t } = useI18n()
const automation = useAutomationStore()
const groups = useGroupsStore()
const profilesById = computed(() => new Map(groups.rows.map((profile) => [profile.id, profile])))

const flowId = ref(0)
const mode = ref<'saved' | 'group' | 'serials' | 'filter'>('group')
const editing = computed(() => props.taskId != null)
const original = ref<TaskDetails | null>(null)
const savedProfileIds = ref<string[]>([])
const savedListOpen = ref(false)
const savedData = ref<Record<string, unknown>[] | null>(null)
const initialValues = ref<Record<string, string | boolean>>({})
const tableVersion = ref(0)
const loadError = ref(false)
const nameError = ref('')
const groupId = ref('')
const serials = ref('')
const distribution = ref<'queue' | 'per_profile'>('queue')
const criteria = ref(defaultProfileCriteria())
const order = ref<'ordered' | 'random'>('ordered')
const threads = ref(1)
const headless = ref<boolean | null>(null)
const missingRow = ref<'skip' | 'random'>('skip')
const name = ref('')
const values = ref<Record<string, string | boolean>>({})
const tablePath = ref<string | null>(null)
const preview = ref<TablePreview | null>(null)
const previewLoading = ref(false)
const tableError = ref('')
const profilesError = ref('')
const paramErrors = ref<Record<string, string>>({})
const errors = ref<FieldError[]>([])
const saving = ref(false)
const loading = ref(false)
const step = ref(1)
const stepHeading = ref<HTMLElement | null>(null)
const threadsError = ref('')
const stepKeys = computed(() => ['dataStep', 'profilesStep', editing.value ? 'saveStep' : 'reviewStep'])
const err = useFieldErrors(() => errors.value)
const KNOWN = ['flow_id', 'profile_ids', 'data_path', 'threads', 'order', 'missing_row', 'distribution', 'name']
const generalErrors = computed(() =>
  errors.value.filter((e) => !KNOWN.includes(e.field)).map((e) => err(e.field) ?? ''),
)
let previewToken = 0

/** Ошибка поля с сервера остаётся до правки этого поля. */
const clearError = (...fields: string[]) => {
  if (errors.value.some((e) => fields.includes(e.field))) errors.value = errors.value.filter((e) => !fields.includes(e.field))
}

const flow = computed(() => automation.flows.find((f) => f.id === flowId.value)
  ?? (flowId.value === original.value?.process?.id ? original.value.process : undefined))
const flowOptions = computed(() =>
  [...automation.flows, ...(original.value?.process && !automation.flows.some((flow) => flow.id === original.value!.process!.id) ? [original.value.process] : [])].map((f) => ({
    value: String(f.id),
    label: f.has_errors ? `${f.name} ${t('automation.taskDialog.hasErrors')}` : f.name,
    disabled: f.has_errors,
  })),
)
const groupOptions = computed(() => groups.groups.map((g) => ({ value: String(g.id), label: g.name })))
const filterGroupOptions = computed(() => [
  { value: 'all', label: t('profiles.filters.allGroups') },
  ...groupOptions.value,
  { value: 'ungrouped', label: t('common.ungrouped') },
])
const filterGroup = computed({
  get: () => String(criteria.value.group),
  set: (value: string) => {
    criteria.value.group = value === 'all' || value === 'ungrouped' ? value : Number(value)
  },
})
const tagNames = computed(() => groups.tags.filter((tag) => tag.profile_count > 0).map((tag) => tag.name))
const matchingProfiles = computed(() => filterProfiles(groups.rows, criteria.value))
const hasCriteria = computed(() => criteria.value.group !== 'all' || criteria.value.label !== 'any'
  || criteria.value.tags.length > 0 || criteria.value.search !== '')
const flowModel = computed({ get: () => String(flowId.value), set: (v: string) => (flowId.value = Number(v)) })
const profileModes = computed<{ value: typeof mode.value; label: string }[]>(() => [
  ...(original.value ? [{ value: 'saved' as const, label: t('automation.taskDialog.savedSelection') }] : []),
  { value: 'group', label: t('automation.taskDialog.byGroup') },
  { value: 'serials', label: t('automation.taskDialog.bySerials') },
  { value: 'filter', label: t('automation.taskDialog.byFilter') },
])
const paramDecls = computed<VarDecl[]>(() => {
  const declared = flow.value?.variables ?? []
  if (!original.value || flowId.value !== (original.value.flow_id ?? -1)) return declared
  const extras = Object.entries(original.value.params).filter(([name]) => !declared.some((v) => v.name === name))
    .map(([name, value]): VarDecl => ({ name, default: null, description: '',
      type: Array.isArray(value) ? 'array' : value !== null && typeof value === 'object' ? 'object'
        : typeof value === 'number' ? 'number' : typeof value === 'boolean' ? 'boolean' : 'string' }))
  return [...declared, ...extras]
})

function resetParams() {
  values.value = Object.fromEntries(
    paramDecls.value.map((v) => {
      const saved = original.value && flowId.value === (original.value.flow_id ?? -1)
      return [v.name, saved && Object.hasOwn(original.value!.params, v.name)
        ? formDefault({ ...v, default: original.value!.params[v.name] }) : v.type === 'boolean' ? formDefault(v) : '']
    }),
  )
  initialValues.value = { ...values.value }
  paramErrors.value = {}
}
watch(flowId, resetParams)
watch(flowId, () => clearError('flow_id'))
watch([mode, groupId, serials, () => JSON.stringify(criteria.value)], () => {
  profilesError.value = ''
  clearError('profile_ids')
})
watch(tablePath, () => clearError('data_path'))
watch(threads, () => clearError('threads'))
watch(threads, () => { threadsError.value = '' })
watch(name, () => { clearError('name'); nameError.value = '' })
watch(distribution, () => clearError('distribution', 'data_path'))
let openToken = 0
/** Диалог сам подгружает нужное: процессы, группы и строки профилей; выбор по умолчанию — после ответа. */
async function prepare() {
  const token = ++openToken
  loading.value = true
  loadError.value = false
  original.value = null
  savedProfileIds.value = []
  savedListOpen.value = false
  step.value = 1
  threadsError.value = ''
  mode.value = 'group'
  distribution.value = 'queue'
  criteria.value = defaultProfileCriteria()
  serials.value = name.value = tableError.value = profilesError.value = ''
  order.value = 'ordered'
  missingRow.value = 'skip'
  threads.value = 1
  headless.value = null
  errors.value = []
  removeTable()
  let details: TaskDetails | null = null
  try {
    const result = await Promise.all([automation.refreshFlows(), groups.loaded ? undefined : groups.refresh(),
      props.taskId != null ? core.getTask(props.taskId) : Promise.resolve(null)])
    details = result[2]
  } catch (e) {
    if (token !== openToken) return
    notify.error(e)
    loadError.value = true
    loading.value = false
    return
  }
  if (token !== openToken) return
  original.value = details
  flowId.value = details ? details.flow_id ?? -1 : props.flowId ?? automation.flows.find((f) => !f.has_errors)?.id ?? 0
  if (details) {
    mode.value = 'saved'
    savedProfileIds.value = [...details.profiles]
    serials.value = details.profiles.flatMap((id) => {
      const row = profilesById.value.get(id)
      return row ? [String(row.serial)] : []
    }).join(', ')
    distribution.value = details.distribution
    savedData.value = details.data
    name.value = details.name
    threads.value = details.threads
    headless.value = details.headless ?? null
    order.value = details.order
    missingRow.value = details.missing_row
  }
  resetParams()
  groupId.value = groups.groups[0] ? String(groups.groups[0].id) : ''
  loading.value = false
}
watch(
  open,
  (isOpen) => {
    if (isOpen) void prepare()
    else openToken++
  },
  { immediate: true },
)

/** Один и тот же набор используется в счётчике, предпросмотре и при запуске. */
const selection = computed<{ ids: string[]; error: string }>(() => {
  if (props.profileIds) return { ids: [...new Set(props.profileIds)], error: props.profileIds.length ? '' : t('automation.taskDialog.emptySerials') }
  if (mode.value === 'saved') {
    const ids = [...new Set(savedProfileIds.value)]
    const unavailable = ids.filter((id) => !profilesById.value.has(id)).length
    return { ids, error: unavailable ? t('automation.taskDialog.unavailableProfiles', { n: unavailable })
      : ids.length ? '' : t('automation.taskDialog.emptySerials') }
  }
  if (mode.value === 'filter') {
    const ids = matchingProfiles.value.map((row) => row.id)
    return { ids, error: ids.length ? '' : t('automation.taskDialog.emptyFilter') }
  }
  if (mode.value === 'group') {
    const ids = groups.rows
      .filter((r) => String(r.group?.id) === groupId.value)
      .sort((a, b) => a.serial - b.serial)
      .map((r) => r.id)
    return { ids, error: ids.length ? '' : t('automation.taskDialog.emptyGroup') }
  }
  const ids: string[] = []
  for (const part of serials.value.split(/[\s,;]+/).filter(Boolean)) {
    const row = groups.rows.find((r) => String(r.serial) === part)
    if (!row) {
      return { ids: [], error: t('automation.taskDialog.noSerial', { n: part }) }
    }
    ids.push(row.id)
  }
  return { ids: [...new Set(ids)], error: ids.length ? '' : t('automation.taskDialog.emptySerials') }
})
function resolveProfiles(): string[] | null {
  profilesError.value = selection.value.error
  return selection.value.error ? null : selection.value.ids
}
const hasTable = computed(() => tablePath.value !== null || savedData.value !== null)
const fileName = computed(() => tablePath.value?.split(/[\\/]/).pop() ?? (savedData.value !== null ? t('automation.taskDialog.savedTable', { n: savedData.value.length }) : ''))
const jobs = computed(() => distribution.value === 'queue' ? (hasTable.value ? preview.value?.matched ?? 0 : 1)
  : hasTable.value ? preview.value?.matched ?? 0 : selection.value.ids.length)
const concurrency = computed(() => Math.min(Math.max(1, Math.trunc(Number(threads.value)) || 1), selection.value.ids.length, jobs.value))
const selectionLabel = computed(() => props.profileIds || mode.value === 'saved' ? t('automation.taskDialog.selectedProfiles')
  : mode.value === 'group' ? groupOptions.value.find((g) => g.value === groupId.value)?.label ?? '—'
    : t(`automation.taskDialog.${mode.value === 'filter' ? 'byFilter' : 'bySerials'}`))

function removeTable() {
  previewToken++
  tableVersion.value++
  savedData.value = null
  tablePath.value = null
  preview.value = null
  previewLoading.value = false
  tableError.value = ''
}
async function chooseTable() {
  try {
    const path = await pickTableFile()
    if (path) {
      removeTable()
      tablePath.value = path
    }
  } catch (e) { notify.error(e) }
}
/** Предпросмотр следует за файлом, профилями и режимом; ответ устаревшего запроса отбрасывается. */
async function runPreview() {
  const token = ++previewToken
  previewLoading.value = false
  // Привязку строк проверяем, когда пользователь выбрал профили и распределение.
  if (step.value === 1) return
  preview.value = null
  tableError.value = ''
  if (!hasTable.value) return
  const ids = resolveProfiles()
  if (!ids) return
  previewLoading.value = true
  try {
    const shared = { distribution: distribution.value, profile_ids: ids, missing_row: missingRow.value }
    const result = tablePath.value ? await core.previewTable({ ...shared, data_path: tablePath.value })
      : await core.previewTableData({ ...shared, data: savedData.value! })
    if (token === previewToken) preview.value = result
  } catch (e) {
    if (token === previewToken) tableError.value = toCoreError(e).message
  } finally {
    if (token === previewToken) previewLoading.value = false
  }
}
watch(
  () => JSON.stringify([
    step.value > 1, tableVersion.value, tablePath.value, savedData.value !== null, props.profileIds, mode.value, savedProfileIds.value, groupId.value, serials.value, missingRow.value, distribution.value,
    mode.value === 'filter' ? matchingProfiles.value.map((row) => row.id) : null,
  ]),
  runPreview,
)
const previewText = computed(() => {
  const p = preview.value
  if (!p) return ''
  if (distribution.value === 'queue') return t('automation.taskDialog.queuePreview', { rows: p.rows, total: p.matched, duplicates: p.duplicates ?? 0 })
  const key = p.key === 'order' ? t('automation.taskDialog.byOrder') : p.key
  const columns = p.columns.join(', ') || '—'
  let text = t('automation.taskDialog.preview', { rows: p.rows, columns, key, matched: p.matched })
  if (p.skipped.length > 0) text += ` · ${t('automation.taskDialog.skipped', { list: p.skipped.join(', ') })}`
  return text
})

/** Параметры по типам; пустые текстовые не попадают, boolean — всегда. */
function buildParams(report = true): Record<string, unknown> | null {
  const params: Record<string, unknown> = {}
  const bad: Record<string, string> = {}
  for (const v of paramDecls.value) {
    const raw = values.value[v.name]
    if (original.value && flowId.value === (original.value.flow_id ?? -1) && raw === initialValues.value[v.name]) {
      if (Object.hasOwn(original.value.params, v.name)) params[v.name] = original.value.params[v.name]
      continue
    }
    const text = typeof raw === 'boolean' ? '' : String(raw)
    if (v.type === 'boolean') params[v.name] = raw
    else if (text.trim() !== '') {
      if (v.type === 'number') {
        const n = parseStrictNumber(text)
        if (n === null) bad[v.name] = t('automation.taskDialog.expectNumber')
        else params[v.name] = n
      } else if (v.type === 'array' || v.type === 'object') {
        try {
          params[v.name] = JSON.parse(text)
        } catch {
          bad[v.name] = t('automation.taskDialog.expectJson')
        }
      } else params[v.name] = text
    }
  }
  if (report) paramErrors.value = bad
  return Object.keys(bad).length > 0 ? null : params
}

function stableJson(value: unknown): string {
  return JSON.stringify(value, (_, item) => item && typeof item === 'object' && !Array.isArray(item)
    ? Object.fromEntries(Object.entries(item).sort(([a], [b]) => a.localeCompare(b))) : item)
}
const workloadChanged = computed(() => original.value !== null && (
  flowId.value !== (original.value.flow_id ?? -1) || distribution.value !== original.value.distribution
  || tablePath.value !== null || stableJson(savedData.value) !== stableJson(original.value.data)
  || stableJson(buildParams(false)) !== stableJson(original.value.params)))

const ready = computed(() => !saving.value && !loading.value && !loadError.value && !!flow.value && !flow.value.has_errors)
const canReview = computed(() => ready.value && tableError.value === ''
  && !previewLoading.value && (!hasTable.value || preview.value !== null)
  && selection.value.error === '')
const canCreate = computed(() => canReview.value && jobs.value > 0)
async function goToStep(value: number) {
  step.value = value
  await nextTick()
  stepHeading.value?.focus({ preventScroll: true })
  const body = stepHeading.value?.closest('.body')
  if (body) body.scrollTop = 0
}
async function next() {
  if (!ready.value) return
  if (step.value === 1 && !buildParams()) return
  if (step.value === 2 && (!resolveProfiles() || !canReview.value)) return
  await goToStep(step.value + 1)
}
async function submit() {
  if (step.value !== 3 || !canCreate.value) return
  const ids = resolveProfiles()
  const params = buildParams()
  if (!ids) { await goToStep(2); return }
  if (!params) { await goToStep(1); return }
  const parallel = Number(threads.value)
  if (editing.value && (name.value.trim() === '' || Array.from(name.value.trim()).length > 64)) {
    nameError.value = t('automation.tasks.nameInvalid')
    await nextTick()
    document.getElementById('task-name')?.focus()
    return
  }
  if (!Number.isInteger(parallel) || parallel < 1 || parallel > 4294967295) {
    threadsError.value = t('automation.taskDialog.threadsInvalid')
    await nextTick()
    document.getElementById('task-threads')?.focus()
    return
  }
  saving.value = true
  try {
    const form: TaskForm = {
      headless: headless.value,
      distribution: distribution.value,
      name: name.value.trim() || null,
      flow_id: flowId.value,
      profile_ids: ids,
      params,
      data_path: tablePath.value,
      threads: parallel,
      order: order.value,
      missing_row: missingRow.value,
    }
    let id: number
    if (props.taskId != null && original.value) {
      id = props.taskId
      await automation.updateTask(id, { ...form, name: name.value.trim(), flow_id: flowId.value === -1 ? null : flowId.value,
        revision: original.value.revision, data: savedData.value })
      notify.info(t('automation.tasks.saved'))
    } else {
      id = await automation.createTask(form)
      notify.info(t('automation.taskDialog.created'))
    }
    open.value = false
    if (editing.value) emit('updated', id)
    else emit('created', id)
  } catch (e) {
    fieldsOrToast(e, errors)
    if (errors.value.some((error) => ['flow_id', 'data_path', 'params'].includes(error.field))) await goToStep(1)
    else if (errors.value.some((error) => ['profile_ids', 'distribution'].includes(error.field))) await goToStep(2)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <UiDialog v-model:open="open" :title="t(editing ? 'automation.tasks.editTitle' : 'automation.taskDialog.title')" :width="760" :busy="saving">
    <ol class="steps" :aria-label="t('automation.taskDialog.stepsLabel')">
      <li v-for="(key, index) in stepKeys" :key="key">
        <button class="step" :class="{ current: step === index + 1, complete: step > index + 1 }"
          :aria-current="step === index + 1 ? 'step' : undefined" :disabled="index + 1 > step || saving"
          @click="goToStep(index + 1)">
          <span class="step-number" aria-hidden="true">{{ index + 1 }}</span>
          <span>{{ t('automation.taskDialog.' + key) }}</span>
        </button>
      </li>
    </ol>
    <div class="step-heading">
      <h2 ref="stepHeading" tabindex="-1">{{ t('automation.taskDialog.' + stepKeys[step - 1]) }}</h2>
      <p class="hint">{{ t(editing && step === 3 ? 'automation.taskDialog.saveHint' : 'automation.taskDialog.stepHint' + step) }}</p>
      <span v-if="step === 1 && profileIds" class="context-selection">{{ t('automation.taskDialog.profilesCount', { n: selection.ids.length }) }}</span>
    </div>
    <div v-if="loading" class="hint">{{ t('automation.taskDialog.loading') }}</div>
    <div v-if="loadError" class="table-result" role="alert">
      <span>{{ t('automation.taskDialog.loadFailed') }}</span>
      <UiButton @click="prepare">{{ t('automation.taskDialog.reload') }}</UiButton>
    </div>
    <template v-if="!loading && !loadError">
    <template v-if="step === 1">
    <UiField :label="t('automation.taskDialog.flow')" :error="err('flow_id')">
      <UiSelect v-model="flowModel" :options="flowOptions" :aria-label="t('automation.taskDialog.flow')" />
      <span v-if="!loading && flowOptions.length === 0" class="hint">{{ t('automation.taskDialog.noFlows') }}</span>
      <span v-if="original && !original.process" class="hint">{{ t('automation.taskDialog.missingProcess') }}</span>
    </UiField>
    <section class="form-section">
      <div class="section-heading">
        <h3>{{ t('automation.taskDialog.inputTable') }}</h3>
        <span class="hint">{{ t('automation.taskDialog.optional') }}</span>
      </div>
      <div class="file-card">
        <div class="file-copy">
          <strong v-if="hasTable" class="file-name" :title="tablePath ?? undefined">{{ fileName }}</strong>
          <span v-else>{{ t('automation.taskDialog.noTable') }}</span>
          <span class="hint">{{ hasTable ? t('automation.taskDialog.checkTableNext') : t('automation.taskDialog.tableFormats') }}</span>
        </div>
        <UiButton @click="chooseTable">{{ t('automation.taskDialog.pickFile') }}</UiButton>
        <UiButton v-if="hasTable" variant="ghost" @click="removeTable">{{ t('automation.taskDialog.removeTable') }}</UiButton>
      </div>
      <p v-if="hasTable" class="hint">{{ t('automation.taskDialog.tablePriority') }}</p>
      <div v-if="tableError || err('data_path')" class="error-text" role="alert">{{ tableError || err('data_path') }}</div>
    </section>
    <section v-if="paramDecls.length" class="form-section">
      <div class="section-heading"><h3>{{ t('automation.taskDialog.commonParams') }}</h3></div>
      <p class="hint">{{ t('automation.taskDialog.paramsHint') }}</p>
      <ParamField v-for="v in paramDecls" :key="flowId + '-' + v.name" v-model="values[v.name]"
        :decl="v" :error="paramErrors[v.name]" :from-table="preview?.columns.includes(v.name)" />
    </section>
    </template>
    <template v-else-if="step === 2">
    <UiField v-if="!profileIds" :label="t('automation.taskDialog.profiles')" :input-id="mode === 'serials' ? 'task-serials' : undefined" :error="profilesError || err('profile_ids')">
      <UiSegmented
        v-model="mode"
        :options="profileModes"
        :aria-label="t('automation.taskDialog.profiles')"
      />
      <div v-if="mode === 'saved'" class="saved-selection">
        <p class="hint">{{ t('automation.taskDialog.savedProfilesHint') }}</p>
        <details @toggle="savedListOpen = ($event.target as HTMLDetailsElement).open">
          <summary>{{ t('automation.taskDialog.selectedProfiles') }}</summary>
          <ul v-if="savedListOpen" class="saved-profile-list">
            <li v-for="id in savedProfileIds" :key="id">
              <span>{{ profilesById.get(id)?.serial ?? '—' }} · {{ profilesById.get(id)?.name ?? id }}</span>
              <UiButton size="icon" variant="ghost" icon="close" :aria-label="t('automation.taskDialog.removeProfile', { id })" @click="savedProfileIds = savedProfileIds.filter((value) => value !== id)" />
            </li>
          </ul>
        </details>
      </div>
      <UiSelect v-else-if="mode === 'group'" v-model="groupId" :options="groupOptions" :aria-label="t('automation.taskDialog.byGroup')" />
      <input v-else-if="mode === 'serials'" id="task-serials" v-model="serials" class="input mono" placeholder="1, 3, 5" autocomplete="off" />
      <div v-else class="profile-filter">
        <div class="grid two">
          <UiField :label="t('columns.group')" input-id="task-filter-group">
            <UiSelect id="task-filter-group" v-model="filterGroup" :options="filterGroupOptions" :aria-label="t('profiles.filters.groupLabel')" />
          </UiField>
          <UiField :label="t('columns.status')" input-id="task-filter-status">
            <ProfileStatusFilter id="task-filter-status" v-model="criteria.label" />
          </UiField>
        </div>
        <UiField :label="t('columns.tags')" input-id="task-filter-tags">
          <TagInput v-model="criteria.tags" :suggestions="tagNames" input-id="task-filter-tags" />
        </UiField>
        <UiField :label="t('profiles.searchLabel')" input-id="task-filter-search">
          <input id="task-filter-search" v-model="criteria.search" class="input" type="search" :placeholder="t('profiles.searchPlaceholder')" autocomplete="off" />
        </UiField>
        <div class="hint">{{ t('automation.taskDialog.filterHint') }}</div>
        <div class="filter-summary">
          <span role="status" data-test="task-profile-matches">{{ t('automation.taskDialog.filterCount', { n: matchingProfiles.length }) }}</span>
          <UiButton size="sm" variant="ghost" :disabled="!hasCriteria" @click="criteria = defaultProfileCriteria()">
            {{ t('automation.taskDialog.resetFilter') }}
          </UiButton>
        </div>
        <div v-if="matchingProfiles.length === 0" class="hint">{{ t('automation.taskDialog.emptyFilter') }}</div>
      </div>
    </UiField>
    <div class="selection-summary" role="status">
      <strong>{{ t('automation.taskDialog.profilesCount', { n: selection.ids.length }) }}</strong>
      <span class="hint">{{ selection.error || t('automation.taskDialog.selectionSnapshot') }}</span>
    </div>
    <fieldset class="form-section distribution">
      <legend>{{ t('automation.taskDialog.distribution') }}</legend>
      <div class="mode-options">
        <label v-for="value in (['queue', 'per_profile'] as const)" :key="value" class="mode-card" :class="{ selected: distribution === value }">
          <input v-model="distribution" type="radio" name="task-distribution" :value="value" />
          <span>
            <strong>{{ t(value === 'queue' ? 'automation.taskDialog.queueModeTitle' : 'automation.taskDialog.profileModeTitle') }}</strong>
            <span class="hint">{{ t(value === 'queue' ? 'automation.taskDialog.queueDescription' : 'automation.taskDialog.profileDescription') }}</span>
          </span>
        </label>
      </div>
      <span v-if="err('distribution')" class="error-text">{{ err('distribution') }}</span>
    </fieldset>
    <div v-if="distribution === 'queue' && !hasTable" class="single-job-note">
      <strong>{{ t('automation.taskDialog.singleJob') }}</strong>
      <span>{{ t('automation.taskDialog.singleJobHint') }}</span>
    </div>
    <div v-if="hasTable" class="table-result" aria-live="polite">
      <strong class="file-name">{{ fileName }}</strong>
      <span v-if="previewLoading" class="hint">{{ t('automation.taskDialog.checkingTable') }}</span>
      <span v-else-if="tableError" class="error-text" role="alert">{{ tableError }}</span>
      <span v-else class="hint">{{ previewText }}</span>
      <UiButton size="sm" variant="ghost" @click="goToStep(1)">{{ t('automation.taskDialog.editData') }}</UiButton>
    </div>
    </template>
    <template v-else>
    <div v-if="editing && workloadChanged && (distribution === 'queue' || original?.distribution === 'queue')" class="single-job-note" role="status">
      {{ t('automation.taskDialog.queueChanges') }}
    </div>
    <div class="run-summary" aria-live="polite" data-test="task-run-summary">
      <div class="metrics">
        <div><strong>{{ jobs }}</strong><span>{{ t('automation.taskDialog.jobsLabel') }}</span></div>
        <div><strong>{{ selection.ids.length }}</strong><span>{{ t('automation.taskDialog.profilesLabel') }}</span></div>
      </div>
      <p>{{ t(distribution === 'queue' ? 'automation.taskDialog.queueSummary' : 'automation.taskDialog.profileSummary', { n: concurrency }) }}</p>
      <span v-if="distribution === 'per_profile' && preview?.skipped.length" class="hint">{{ t('automation.taskDialog.skippedSummary', { n: preview.skipped.length }) }}</span>
      <span v-if="!previewLoading && jobs === 0" class="hint">{{ t('automation.taskDialog.noJobsHint') }}</span>
      <span v-if="previewLoading" class="hint">{{ t('automation.taskDialog.checkingTable') }}</span>
      <span v-if="tableError" class="error-text" role="alert">{{ tableError }}</span>
    </div>
    <dl class="review-details">
      <div>
        <dt>{{ t('automation.taskDialog.flow') }}</dt>
        <dd>{{ flow?.name }}<UiButton size="sm" variant="ghost" @click="goToStep(1)">{{ t('automation.taskDialog.edit') }}</UiButton></dd>
      </div>
      <div>
        <dt>{{ t('automation.taskDialog.dataLabel') }}</dt>
        <dd><span class="file-name" :title="tablePath ?? undefined">{{ fileName || t('automation.taskDialog.commonParams') }}</span></dd>
      </div>
      <div>
        <dt>{{ t('automation.taskDialog.profiles') }}</dt>
        <dd>{{ selectionLabel }}<UiButton size="sm" variant="ghost" @click="goToStep(2)">{{ t('automation.taskDialog.edit') }}</UiButton></dd>
      </div>
      <div>
        <dt>{{ t('automation.taskDialog.distribution') }}</dt>
        <dd>{{ t(distribution === 'queue' ? 'automation.taskDialog.queueModeTitle' : 'automation.taskDialog.profileModeTitle') }}</dd>
      </div>
    </dl>
    <UiField :label="t(editing ? 'automation.tasks.columns.name' : 'automation.taskDialog.name')" input-id="task-name" :error="nameError || err('name')">
      <input id="task-name" v-model="name" class="input" autocomplete="off" :placeholder="flow?.name" />
    </UiField>
    <BrowserModeField v-model="headless" inherit :inherited-headless="flow?.headless ?? original?.flow?.settings?.headless ?? false" />
    <UiField :label="t('automation.taskDialog.threads')" input-id="task-threads"
      :hint="t('automation.taskDialog.parallelHint') + (jobs === 1 ? ' ' + t('automation.taskDialog.singleThreadHint') : '')" :error="threadsError || err('threads')">
      <input id="task-threads" v-model="threads" class="input parallel-input" type="number" min="1" />
    </UiField>
    <details class="advanced">
      <summary>{{ t('automation.taskDialog.advanced') }}</summary>
      <UiField :label="t(distribution === 'queue' ? 'automation.taskDialog.queueOrder' : 'automation.taskDialog.profileOrder')" :error="err('order')">
        <UiSegmented
          v-model="order"
          :options="[
            { value: 'ordered', label: t('automation.taskDialog.ordered') },
            { value: 'random', label: t('automation.taskDialog.random') },
          ]"
          :aria-label="t(distribution === 'queue' ? 'automation.taskDialog.queueOrder' : 'automation.taskDialog.order')"
        />
      </UiField>
    <UiField v-if="distribution === 'per_profile' && hasTable" :label="t('automation.taskDialog.missingRow')" :error="err('missing_row')">
      <UiSegmented
        v-model="missingRow"
        :options="[
          { value: 'skip', label: t('automation.taskDialog.skip') },
          { value: 'random', label: t('automation.taskDialog.randomRow') },
        ]"
        :aria-label="t('automation.taskDialog.missingRow')"
      />
    </UiField>
    </details>
    </template>
    </template>
    <div v-for="text in generalErrors" :key="text" class="error-text" role="alert">{{ text }}</div>
    <template #footer>
      <UiButton class="cancel" variant="ghost" :disabled="saving" @click="open = false">{{ t('common.cancel') }}</UiButton>
      <UiButton v-if="step > 1" :disabled="saving" @click="goToStep(step - 1)">{{ t('automation.taskDialog.back') }}</UiButton>
      <UiButton v-if="step < 3" variant="primary" :disabled="!ready || (step === 2 && !canReview)" @click="next">
        {{ t(step === 1 ? 'automation.taskDialog.toProfiles' : 'automation.taskDialog.toReview') }}
      </UiButton>
      <UiButton v-else variant="primary" :loading="saving" :disabled="!canCreate" @click="submit">
        {{ t(editing ? 'common.save' : 'automation.taskDialog.submit') }}
      </UiButton>
    </template>
  </UiDialog>
</template>

<style scoped>
.steps { display: flex; gap: 8px; margin: 0 0 8px; padding: 0 0 20px; list-style: none; border-bottom: 1px solid var(--border); }
.steps li { flex: 1; min-width: 0; }
.step { display: flex; align-items: center; gap: 8px; width: 100%; padding: 4px 0; border: 0; background: transparent; color: var(--text-2); text-align: left; font-size: 12px; line-height: 18px; cursor: pointer; }
.step:disabled { cursor: default; }
.step.current { color: var(--accent-ink); font-weight: 600; }
.step.complete { color: var(--text); }
.step-number { display: grid; place-items: center; flex: 0 0 28px; width: 28px; height: 28px; border: 1px solid var(--border); border-radius: 50%; font-variant-numeric: tabular-nums; }
.current .step-number { background: var(--accent); border-color: var(--accent); color: var(--text-inverse); }
.complete .step-number { background: var(--accent-soft); border-color: var(--accent-soft); color: var(--accent-ink); }
.step:focus-visible, summary:focus-visible { outline: 2px solid var(--focus); outline-offset: 3px; }
.step-heading { display: flex; flex-direction: column; gap: 6px; margin-bottom: 6px; }
.step-heading h2 { margin: 0; font-size: 22px; font-weight: 600; line-height: 28px; }
.step-heading h2:focus { outline: none; }
.step-heading p, .form-section p, .run-summary p { margin: 0; }
.context-selection { align-self: flex-start; margin-top: 4px; padding: 4px 8px; border-radius: var(--radius-control); background: var(--muted-bg); font-size: 13px; }
.form-section { display: flex; flex-direction: column; gap: 14px; min-width: 0; margin: 6px 0 0; padding: 18px 0 0; border: 0; border-top: 1px solid var(--border); }
.section-heading { display: flex; align-items: baseline; gap: 8px; }
.section-heading h3, .distribution legend { margin: 0; font-size: 14px; font-weight: 600; }
.file-card { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; padding: 16px; border: 1px dashed var(--border-strong); border-radius: var(--radius-card); }
.file-copy { display: flex; flex: 1 1 220px; flex-direction: column; gap: 5px; min-width: 0; }
.file-name { min-width: 0; overflow-wrap: anywhere; }
.selection-summary { display: flex; flex-direction: column; gap: 4px; }
.distribution { padding-top: 12px; }
.distribution legend { padding: 0 8px 0 0; }
.mode-options { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
.mode-card { display: flex; align-items: flex-start; gap: 10px; padding: 16px; border: 1px solid var(--border-strong); border-radius: var(--radius-card); cursor: pointer; }
.mode-card.selected { border-color: var(--accent-ink); background: var(--accent-soft); }
.mode-card input { flex-shrink: 0; margin: 3px 0 0; accent-color: var(--accent); }
.mode-card input:focus-visible { outline: 2px solid var(--focus); outline-offset: 3px; }
.mode-card > span { display: flex; flex-direction: column; gap: 6px; }
.mode-card strong { font-size: 14px; font-weight: 600; }
.single-job-note, .table-result { display: flex; flex-direction: column; gap: 6px; padding: 16px; border-radius: var(--radius-card); background: var(--muted-bg); font-size: 13px; line-height: 20px; }
.table-result .btn { align-self: flex-start; margin-left: -12px; }
.run-summary { display: flex; flex-direction: column; gap: 12px; padding: 16px; border: 1px solid var(--accent-soft); border-radius: var(--radius-card); background: var(--accent-soft); }
.metrics { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; }
.metrics > div { display: flex; flex-direction: column; gap: 2px; }
.metrics strong { font-size: 30px; font-weight: 600; line-height: 36px; font-variant-numeric: tabular-nums; }
.metrics span, .run-summary p { font-size: 13px; line-height: 20px; }
.review-details { display: flex; flex-direction: column; margin: 0; }
.review-details > div { display: grid; grid-template-columns: 160px minmax(0, 1fr); align-items: baseline; gap: 12px; padding: 8px 0; border-bottom: 1px solid var(--border); }
.review-details dt { color: var(--text-2); font-size: 13px; }
.review-details dd { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; min-width: 0; margin: 0; overflow-wrap: anywhere; }
.review-details .btn { height: 28px; }
.parallel-input { max-width: 140px; }
.advanced { border-top: 1px solid var(--border); padding-top: 14px; }
.advanced summary { padding: 4px 0; color: var(--text-2); cursor: pointer; }
.advanced > .field { margin-top: 14px; }
.cancel { margin-right: auto; }
.saved-selection p { margin: 4px 0 10px; }
.saved-selection summary { cursor: pointer; }
.saved-profile-list { display: flex; flex-direction: column; gap: 4px; margin: 10px 0 0; padding: 0; list-style: none; }
.saved-profile-list li { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.saved-profile-list li > span { min-width: 0; overflow-wrap: anywhere; }
.profile-filter {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
}

.filter-summary {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  font-size: 13px;
}

@media (max-width: 600px) {
  .grid.two { grid-template-columns: minmax(0, 1fr); }
  .mode-options { grid-template-columns: minmax(0, 1fr); }
  .step { flex-direction: column; align-items: flex-start; gap: 6px; }
  .review-details > div { grid-template-columns: 100px minmax(0, 1fr); }
}
</style>
