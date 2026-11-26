import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { core } from '../api/core'
import type { GroupForm, GroupRecord, ListQuery, ProfileRow, SupervisorEvent, TagRecord } from '../api/types'
import { reportError } from '../lib/notify'
import { createRefresher } from '../lib/refresher'
import { useProfilesStore } from './profiles'

/** Все профили без фильтров: из них считаются «Состояние» групп и боковая панель экрана «Группы». */
const ALL_PROFILES: ListQuery = {
  search: '',
  sort: 'serial',
  descending: false,
  status: null,
  group_id: null,
  ungrouped: false,
  tag: null,
}

/** Группы, теги (подсказки и фильтр) и полный список профилей. */
export const useGroupsStore = defineStore('groups', () => {
  const groups = ref<GroupRecord[]>([])
  const ungrouped = ref(0)
  const tags = ref<TagRecord[]>([])
  const rows = ref<ProfileRow[]>([])
  const loaded = ref(false)
  const quickAccess = computed(() => groups.value.filter((group) => group.quick_access))

  const refresher = createRefresher(
    () => Promise.all([core.listGroups(), core.listTags(), core.listProfiles(ALL_PROFILES)]),
    ([groupList, tagList, profileList]) => {
      groups.value = groupList.groups
      ungrouped.value = groupList.ungrouped
      tags.value = tagList
      rows.value = profileList.rows
      loaded.value = true
      useProfilesStore().resetMissing(
        groupList.groups.map((group) => group.id),
        tagList.map((tag) => tag.name),
      )
    },
  )

  /** Статус меняет только сводку «Состояние»: число профилей в группах от него не зависит. */
  function applyEvent(event: SupervisorEvent) {
    if (event.kind !== 'status') return
    const row = rows.value.find((r) => r.id === event.profile_id)
    if (row) row.status = event.status
  }

  function byId(id: number): GroupRecord | undefined {
    return groups.value.find((group) => group.id === id)
  }

  /** Операция удалась, даже если обновление списков потом не прошло: та ошибка — отдельным уведомлением. */
  async function afterWrite<T>(write: Promise<T>): Promise<T> {
    const result = await write
    await refresher.refresh().catch(reportError)
    return result
  }

  const create = (form: GroupForm) => afterWrite(core.createGroup(form))
  const update = (id: number, form: GroupForm) => afterWrite(core.updateGroup(id, form))
  const remove = (id: number) => afterWrite(core.deleteGroup(id))

  async function changeTags(id: string, add: string[], remove: string[]) {
    const tags = await core.changeProfileTags(id, add, remove)
    const profiles = useProfilesStore()
    for (const row of [...profiles.rows, ...rows.value]) if (row.id === id) row.tags = [...tags]
    profiles.scheduleRefresh(0)
    // Общий справочник, счётчики тегов и фильтры перечитываются после успешной записи.
    await refresher.refresh().catch(reportError)
    return tags
  }

  return {
    groups,
    ungrouped,
    tags,
    rows,
    loaded,
    quickAccess,
    refresh: refresher.refresh,
    scheduleRefresh: refresher.schedule,
    applyEvent,
    byId,
    create,
    update,
    remove,
    changeTags,
  }
})
