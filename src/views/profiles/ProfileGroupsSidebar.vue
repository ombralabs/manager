<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useId, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import AppIcon from '../../components/AppIcon.vue'
import UiButton from '../../components/UiButton.vue'
import { PREF_PROFILE_GROUPS_COLLAPSED, readPref, writePref } from '../../lib/prefs'
import { fold } from '../../lib/tags'
import { useGroupsStore } from '../../stores/groups'
import { useProfilesStore } from '../../stores/profiles'
import ProfileGroupDialog from './ProfileGroupDialog.vue'

const { t } = useI18n()
const groups = useGroupsStore()
const profiles = useProfilesStore()
const contentId = useId()
const query = ref('')
const creating = ref(false)
const media = window.matchMedia('(max-width: 1180px)')
const narrow = ref(media.matches)
const preference = ref<boolean | null>(readPref(PREF_PROFILE_GROUPS_COLLAPSED, (value) => typeof value === 'boolean' ? value : null, null))
const collapsed = computed(() => preference.value ?? narrow.value)
watch(preference, (value) => writePref(PREF_PROFILE_GROUPS_COLLAPSED, value))
const onResize = () => { narrow.value = media.matches }
onMounted(() => media.addEventListener('change', onResize))
onBeforeUnmount(() => media.removeEventListener('change', onResize))

const matches = computed(() => groups.groups.filter((group) => fold(group.name).includes(fold(query.value))))
const total = computed(() => groups.loaded ? groups.rows.length : profiles.total)
const current = computed(() => typeof profiles.filters.group === 'number' ? groups.byId(profiles.filters.group)?.name
  : profiles.filters.group === 'ungrouped' ? t('common.ungrouped') : t('profiles.groupSidebar.all'))

function created(id: number) {
  query.value = ''
  profiles.filters.group = id
}
</script>

<template>
  <aside :class="['profile-groups', { collapsed }]" :aria-label="t('groups.title')">
    <header class="groups-head">
      <h2 v-if="!collapsed">{{ t('groups.title') }}</h2>
      <UiButton v-if="!collapsed" size="icon" variant="ghost" icon="plus" :icon-size="16" :aria-label="t('groups.new')" :title="t('groups.new')" @click="creating = true" />
      <UiButton size="icon" variant="ghost" :icon="collapsed ? 'panelExpand' : 'panelCollapse'" :icon-size="18"
        :aria-label="t(collapsed ? 'profiles.groupSidebar.expand' : 'profiles.groupSidebar.collapse')" :aria-expanded="!collapsed" :aria-controls="contentId"
        :title="collapsed ? `${t('profiles.groupSidebar.expand')} · ${current}` : t('profiles.groupSidebar.collapse')" @click="preference = !collapsed" />
    </header>
    <div v-if="!collapsed" :id="contentId" class="groups-content">
      <label class="group-search">
        <AppIcon name="search" :size="16" />
        <input v-model="query" type="search" :placeholder="t('profiles.groupSidebar.search')" :aria-label="t('profiles.groupSidebar.search')" />
      </label>
      <nav class="group-list" :aria-label="t('profiles.groupSidebar.navigation')">
        <div class="system-groups">
          <button type="button" :class="['group-row', { active: profiles.filters.group === 'all' }]" :aria-pressed="profiles.filters.group === 'all'" @click="profiles.filters.group = 'all'">
            <AppIcon name="profiles" :size="18" /><span class="group-name">{{ t('profiles.groupSidebar.all') }}</span><span class="group-count">{{ total }}</span>
          </button>
          <button type="button" :class="['group-row', { active: profiles.filters.group === 'ungrouped' }]" :aria-pressed="profiles.filters.group === 'ungrouped'" @click="profiles.filters.group = 'ungrouped'">
            <AppIcon name="folder" :size="18" /><span class="group-name">{{ t('common.ungrouped') }}</span><span class="group-count">{{ groups.ungrouped }}</span>
          </button>
        </div>
        <div class="user-groups">
          <button v-for="group in matches" :key="group.id" type="button" :data-group-id="group.id" :class="['group-row', { active: profiles.filters.group === group.id }]"
            :aria-pressed="profiles.filters.group === group.id" :title="group.name" @click="profiles.filters.group = group.id">
            <AppIcon name="folder" :size="18" :style="{ color: group.color }" /><span class="group-name">{{ group.name }}</span><span class="group-count">{{ group.profile_count }}</span>
          </button>
          <p v-if="!matches.length" class="groups-empty">{{ t(!groups.loaded ? 'common.loading' : query ? 'profiles.groupSidebar.noMatches' : 'profiles.groupSidebar.empty') }}</p>
        </div>
      </nav>
      <RouterLink class="manage-groups" to="/groups"><AppIcon name="settings" :size="16" />{{ t('profiles.groupSidebar.manage') }}</RouterLink>
    </div>
    <ProfileGroupDialog v-if="creating" v-model:open="creating" @created="created($event.id)" />
  </aside>
</template>

<style scoped>
.profile-groups { display: flex; flex-direction: column; flex-shrink: 0; width: 208px; min-height: 0; padding: 20px 12px 12px; border-right: 1px solid var(--border); background: var(--surface); }
.groups-head { display: flex; align-items: center; gap: 2px; flex-shrink: 0; min-height: 32px; margin: 0 0 16px; }
.groups-head h2 { flex: 1; margin: 0; padding-left: 8px; color: var(--text-2); font-size: 12px; font-weight: 500; }
.groups-head .btn.square { width: 32px; height: 32px; color: var(--text-2); }
.groups-content { display: flex; flex-direction: column; gap: 16px; flex: 1; min-height: 0; }
.group-search { display: flex; align-items: center; gap: 8px; flex-shrink: 0; min-height: 36px; padding: 0 8px; border: 1px solid var(--border); border-radius: 6px; color: var(--text-2); }
.group-search:focus-within { border-color: var(--accent); }
.group-search input { min-width: 0; width: 100%; padding: 0; border: 0; outline: 0; background: transparent; font: inherit; font-size: 12px; color: var(--text); }
.group-search input::placeholder { color: var(--text-2); }
.group-list { flex: 1; min-height: 0; overflow-y: auto; }
.system-groups { padding-bottom: 12px; margin-bottom: 12px; border-bottom: 1px solid var(--border); }
.group-row { display: flex; align-items: center; gap: 8px; width: 100%; min-height: 40px; margin-bottom: 2px; padding: 8px; border: 0; border-radius: 6px; background: transparent; color: var(--text); font: inherit; font-size: 12px; text-align: left; cursor: pointer; }
.group-row > .icon { flex-shrink: 0; color: var(--text-2); }
.group-row:hover { background: var(--muted-bg); }
.group-row.active { background: var(--accent-soft); color: var(--accent-ink); }
.group-row.active > .icon { color: var(--accent-ink); }
.group-row:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
.group-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.group-count { flex-shrink: 0; color: var(--text-2); font-size: 11px; font-variant-numeric: tabular-nums; }
.groups-empty { margin: 0; padding: 8px; font-size: 12px; line-height: 18px; color: var(--text-2); }
.manage-groups { display: flex; align-items: center; gap: 8px; flex-shrink: 0; min-height: 36px; padding: 8px; border-top: 1px solid var(--border); color: var(--text-2); font-size: 12px; text-decoration: none; }
.manage-groups:hover { color: var(--text); }
.manage-groups:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; border-radius: 4px; }
.profile-groups.collapsed { width: 44px; padding-inline: 6px; }
.collapsed .groups-head { margin-inline: 0; justify-content: center; }
</style>
