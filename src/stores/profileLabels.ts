import { defineStore } from 'pinia'
import { ref } from 'vue'
import { core } from '../api/core'
import type { ProfileLabel } from '../api/types'
import { createRefresher } from '../lib/refresher'
import { useGroupsStore } from './groups'
import { useProfilesStore } from './profiles'

export const useProfileLabelsStore = defineStore('profileLabels', () => {
  const labels = ref<ProfileLabel[]>([])
  const loaded = ref(false)
  let revision = 0

  function syncRows(removedId?: number) {
    const byId = new Map(labels.value.map((label) => [label.id, label]))
    const profiles = useProfilesStore()
    for (const row of [...profiles.rows, ...useGroupsStore().rows]) {
      if (row.label) row.label = byId.get(row.label.id) ?? (loaded.value || row.label.id === removedId ? null : row.label)
    }
    if (typeof profiles.filters.label === 'number' && !byId.has(profiles.filters.label)
      && (loaded.value || profiles.filters.label === removedId)) profiles.filters.label = 'any'
  }

  const refresher = createRefresher(async () => ({ version: revision, list: await core.listProfileLabels() }), ({ version, list }) => {
    if (version !== revision) return
    labels.value = list
    loaded.value = true
    syncRows()
  })
  const refresh = refresher.refresh
  function scheduleRefresh() { if (loaded.value) refresher.schedule(100) }

  async function assign(id: string, labelId: number | null) {
    const saved = await core.setProfileLabel(id, labelId)
    const profiles = useProfilesStore()
    for (const row of [...profiles.rows, ...useGroupsStore().rows]) if (row.id === id) row.label = saved
    profiles.scheduleRefresh(0)
    return saved
  }

  async function save(id: number | null, name: string, color: string) {
    const saved = await core.saveProfileLabel(id, name, color)
    revision++
    const index = labels.value.findIndex((label) => label.id === saved.id)
    if (index < 0) labels.value.push(saved)
    else labels.value[index] = saved
    const profiles = useProfilesStore()
    syncRows()
    profiles.scheduleRefresh(0)
    return saved
  }

  async function remove(id: number) {
    await core.deleteProfileLabel(id)
    revision++
    labels.value = labels.value.filter((label) => label.id !== id)
    const profiles = useProfilesStore()
    syncRows(id)
    profiles.scheduleRefresh(0)
  }

  return { labels, loaded, refresh, scheduleRefresh, assign, save, remove }
})
