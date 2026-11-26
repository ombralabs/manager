<script setup lang="ts">
import {
  DropdownMenuContent,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuTrigger,
  PopoverContent,
  PopoverPortal,
  PopoverRoot,
  PopoverTrigger,
} from 'reka-ui'
import { computed, reactive, ref, watch } from 'vue'
import { I18nT, useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { core } from '../../api/core'
import { toCoreError } from '../../api/errors'
import type { FieldError, GroupForm, GroupRecord, ProfileRow } from '../../api/types'
import AppIcon from '../../components/AppIcon.vue'
import ColorPalette from '../../components/ColorPalette.vue'
import GroupMenuItems from '../../components/GroupMenuItems.vue'
import StatusPill from '../../components/StatusPill.vue'
import UiButton from '../../components/UiButton.vue'
import UiField from '../../components/UiField.vue'
import { showBulkReport } from '../../lib/bulkReport'
import { useFieldErrors } from '../../lib/errorText'
import { setLabel } from '../../lib/extensions'
import { countStatuses, extSetCounts, nextGroupColor, panelProfiles } from '../../lib/groups'
import { notify } from '../../lib/notify'
import { useClearFieldErrors } from '../../lib/useClearFieldErrors'
import { useExtensionsStore } from '../../stores/extensions'
import { useGroupsStore } from '../../stores/groups'
import { useProfilesStore } from '../../stores/profiles'
import StateSummary from './StateSummary.vue'

const props = defineProps<{ group: GroupRecord | null; rows: ProfileRow[] }>()
const emit = defineEmits<{ close: []; saved: [group: GroupRecord]; requestDelete: [group: GroupRecord] }>()

const { t } = useI18n()
const router = useRouter()
const groups = useGroupsStore()
const profiles = useProfilesStore()
const extensions = useExtensionsStore()

/** Черновик формы. Панель пересоздаётся при смене группы (`:key` в `GroupsView`). */
const draft = reactive<GroupForm>(
  props.group
    ? { name: props.group.name, color: props.group.color, note: props.group.note, quick_access: props.group.quick_access }
    : { name: '', color: nextGroupColor(groups.groups), note: '', quick_access: false },
)
// Звезда в таблице сохраняется сразу; черновик берёт новое значение, чтобы «Сохранить» его не откатил.
watch(
  () => props.group?.quick_access,
  (value) => {
    if (value !== undefined) draft.quick_access = value
  },
)

const errors = ref<FieldError[]>([])
const nameTaken = ref(false)
const err = useFieldErrors(() => errors.value)
const nameError = computed(() => (nameTaken.value ? t('errors.GroupExists') : err('group_name')))
const fieldError = computed(() => nameError.value ?? err('group_color'))
watch(
  () => draft.name,
  () => {
    nameTaken.value = false
  },
)
useClearFieldErrors(errors, () => draft, (field) => (field === 'group_name' ? draft.name : ''))

const saving = ref(false)
const moving = ref(false)
const paletteOpen = ref(false)

const title = computed(() =>
  props.group ? t('groups.panel.title', { name: props.group.name }) : t('groups.panel.newTitle'),
)
const members = computed(() => panelProfiles(props.rows))
const restCounts = computed(() => countStatuses(members.value.rest))
/** Наборы назначены профилям, а не группе: у профилей одной группы они бывают разными. Пока библиотека грузится — «…». */
const extSets = computed(() =>
  extSetCounts(props.rows).map(({ id, n }) => {
    const set = extensions.categoryById(id)
    return { id, n, set, name: set ? setLabel(set) : '…', list: extensions.extensionsOf(set) }
  }),
)

async function save() {
  const form: GroupForm = { ...draft, name: draft.name.trim(), note: draft.note.trim() }
  if (form.name === '') {
    errors.value = [{ field: 'group_name', code: 'required' }]
    return
  }
  saving.value = true
  try {
    const saved = props.group ? await groups.update(props.group.id, form) : await groups.create(form)
    notify.info(t(props.group ? 'groups.saved' : 'groups.created'))
    emit('saved', saved)
  } catch (e) {
    const error = toCoreError(e)
    if (error.code === 'GroupExists') nameTaken.value = true
    else if (error.code === 'Validation') errors.value = error.fields
    else notify.error(error)
  } finally {
    saving.value = false
  }
}

function openInList() {
  if (!props.group) return
  profiles.openGroup(props.group.id)
  void router.push('/profiles')
}

/** Все профили группы — в другую группу или в «Без группы». */
async function moveTo(target: number | null) {
  moving.value = true
  try {
    showBulkReport('move', await core.moveProfiles(props.rows.map((row) => row.id), target))
  } catch (e) {
    notify.error(e)
  } finally {
    moving.value = false
  }
  await groups.refresh().catch((e) => notify.error(e))
}
</script>

<template>
  <aside class="panel card" :aria-label="title">
    <header class="panel-head">
      <h2 class="section-title title" :title="title">{{ title }}</h2>
      <button
        type="button"
        class="quick"
        :aria-pressed="draft.quick_access"
        @click="draft.quick_access = !draft.quick_access"
      >
        <AppIcon :name="draft.quick_access ? 'starFilled' : 'star'" :size="16" />{{ t('groups.panel.quickAccess') }}
      </button>
      <UiButton size="icon" variant="ghost" icon="close" :aria-label="t('common.close')" @click="emit('close')" />
    </header>

    <UiField :label="t('groups.panel.nameAndColor')" input-id="group-name" :error="fieldError">
      <div class="name-row">
        <PopoverRoot v-model:open="paletteOpen">
          <PopoverTrigger as-child>
            <button type="button" class="input color-button" :aria-label="t('groups.panel.color')" :title="t('groups.panel.color')">
              <span class="dot" :style="{ background: draft.color }" />
            </button>
          </PopoverTrigger>
          <PopoverPortal>
            <PopoverContent align="start" :side-offset="6" class="popover palette-popover">
              <ColorPalette v-model="draft.color" @pick="paletteOpen = false" />
            </PopoverContent>
          </PopoverPortal>
        </PopoverRoot>
        <input
          id="group-name"
          v-model="draft.name"
          class="input"
          :class="{ invalid: nameError }"
          maxlength="50"
          :placeholder="t('groups.panel.namePlaceholder')"
        />
      </div>
    </UiField>

    <UiField :label="t('groups.panel.note')" input-id="group-note">
      <textarea id="group-note" v-model="draft.note" class="input group-note" rows="1" />
    </UiField>

    <template v-if="group">
      <div class="members">
        <div class="members-head">
          <span class="label">{{ t('groups.panel.profiles', { n: rows.length }) }}</span>
          <button v-if="rows.length > 0" type="button" class="link" @click="openInList">{{ t('groups.openInList') }}</button>
        </div>
        <div v-if="members.shown.length > 0" class="members-box">
          <ul class="members-list">
            <li v-for="member in members.shown" :key="member.id" class="member" data-test="panel-profile">
              <span class="member-name">{{ member.name }}</span>
              <StatusPill :state="member.status.state" compact />
            </li>
          </ul>
          <button v-if="members.rest.length > 0" type="button" class="link more" @click="openInList">
            <I18nT keypath="groups.panel.more" :plural="members.rest.length" scope="global">
              <template #summary><StateSummary :counts="restCounts" /></template>
            </I18nT>
          </button>
        </div>
      </div>

      <div v-if="extSets.length > 0" class="members">
        <span class="label">{{ t('groups.panel.extensions') }}</span>
        <ul class="members-box ext-sets">
          <li v-for="item in extSets" :key="item.id" class="ext-set" data-test="panel-ext-set">
            <div class="ext-set-head">{{ t('groups.panel.extSet', { name: item.name, n: item.n }, item.n) }}</div>
            <ul v-if="item.list.length > 0" class="ext-list">
              <li v-for="ext in item.list" :key="ext.id" class="ext">
                <span class="ext-name"
                  ><span class="member-name">{{ ext.name }}</span
                  ><span v-if="ext.gateway_risk" class="risk" :title="t('extensions.risk')"
                    ><AppIcon name="alert" :size="14" /><span class="sr-only">{{ t('extensions.risk') }}</span></span
                  ></span
                ><span class="hint mono">{{ ext.version }}</span>
              </li>
            </ul>
            <div v-else-if="item.set" class="hint">{{ t(item.id === 0 ? 'groups.panel.extSetNoneEnabled' : 'groups.panel.extSetEmpty') }}</div>
          </li>
        </ul>
      </div>

      <div class="buttons">
        <DropdownMenuRoot v-if="rows.length > 0">
          <DropdownMenuTrigger as-child>
            <UiButton class="grow" :loading="moving">{{ t('groups.panel.move') }}</UiButton>
          </DropdownMenuTrigger>
          <DropdownMenuPortal>
            <DropdownMenuContent align="start" :side-offset="4" class="popover">
              <GroupMenuItems :groups="groups.groups" :exclude="group.id" :disabled="moving" @select="moveTo" />
            </DropdownMenuContent>
          </DropdownMenuPortal>
        </DropdownMenuRoot>
        <UiButton variant="danger" @click="emit('requestDelete', group)">{{ t('groups.delete') }}</UiButton>
      </div>
      <p class="foot">{{ t('groups.deleteText') }}</p>
    </template>

    <UiButton variant="primary" :loading="saving" @click="save">{{ t('common.save') }}</UiButton>
  </aside>
</template>

<style scoped>
/* Панель по высоте содержимого: «Сохранить» видна целиком; в низком окне — прокрутка. */
.panel {
  display: flex;
  flex-direction: column;
  gap: 14px;
  flex-shrink: 0;
  align-self: flex-start;
  width: 360px;
  max-height: 100%;
  padding: 20px;
  overflow: auto;
}

.panel-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  height: 32px;
}

.title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.quick {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  height: 32px;
  margin-left: auto;
  padding: 0 10px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--text-2);
  font-size: 13px;
  font-weight: 600;
  white-space: nowrap;
  cursor: pointer;
}

.quick[aria-pressed='true'] {
  background: var(--accent-soft);
  color: var(--accent-ink);
}

/* Звезда холста: заливка акцентом, контур цвета текста кнопки. */
.quick[aria-pressed='true'] .icon.filled {
  fill: var(--accent);
  stroke: currentColor;
  stroke-width: 1.6;
}

.quick:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}

.name-row {
  display: flex;
  gap: 8px;
}

.group-note {
  height: var(--control-h);
  resize: none;
}

.color-button {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: var(--control-h);
  padding: 0;
  cursor: pointer;
}

.dot {
  display: block;
  width: 18px;
  height: 18px;
  border-radius: 50%;
}

.members {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.members-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.label {
  font-size: 13px;
  font-weight: 600;
}

.members-box {
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: 10px;
}

.members-list:last-child .member:last-child {
  border-bottom: 0;
}

.members-list {
  margin: 0;
  padding: 0;
  list-style: none;
}

.member {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  height: 44px;
  padding: 0 12px;
  border-bottom: 1px solid var(--row-border);
  font-size: 13px;
  font-weight: 500;
}

.member-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.more {
  display: block;
  width: 100%;
  height: 44px;
  border-radius: 0;
  background: var(--more-bg);
  font-size: 13px;
  text-align: center;
  text-decoration: none;
}

/* members-box обрезает по overflow: кольцо фокуса рисуем внутри. */
.more:focus-visible {
  outline-offset: -2px;
}

.ext-sets {
  margin: 0;
  padding: 0;
  list-style: none;
}

.ext-set {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--row-border);
  font-size: 13px;
}

.ext-set:last-child {
  border-bottom: 0;
}

.ext-set-head {
  font-weight: 600;
}

.ext-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.ext {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.ext-name {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.risk {
  display: inline-flex;
  flex-shrink: 0;
  color: var(--warn-text);
}

.buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
}

.buttons :deep(.btn) {
  padding: 0 10px;
}

.grow {
  flex-grow: 1;
}

.foot {
  margin: 0;
  font-size: 12px;
  line-height: 17px;
  color: var(--text-2);
}
</style>

<style>
/* `PopoverContent` уходит в портал без атрибута области: правило без `scoped`, составной селектор перекрывает `.popover`. */
.popover.palette-popover {
  min-width: 0;
  padding: 12px;
}
</style>
