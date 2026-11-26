<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { apiFailureText } from '../lib/errorText'
import { engineMajor } from '../lib/format'
import { PREF_SIDEBAR_COLLAPSED, readPref, writePref } from '../lib/prefs'
import { useGroupsStore } from '../stores/groups'
import { useProfilesStore } from '../stores/profiles'
import { useAppStore } from '../stores/app'
import { useSettingsStore } from '../stores/settings'
import UiTooltip from './UiTooltip.vue'
import AppIcon from './AppIcon.vue'
import type { IconName } from './icons'
import OmbraMark from './OmbraMark.vue'

/** Пункт меню разделов. */
interface NavItem {
  label: string
  icon: IconName
  to: string
}

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const groups = useGroupsStore()
const profiles = useProfilesStore()
const collapsed = ref(readPref(PREF_SIDEBAR_COLLAPSED, (value) => typeof value === 'boolean' ? value : null, true))
watch(collapsed, (value) => writePref(PREF_SIDEBAR_COLLAPSED, value))

function openGroup(id: number) {
  profiles.openGroup(id)
  if (route.path !== '/profiles') void router.push('/profiles')
}
const app = useAppStore()
const settings = useSettingsStore()

/**
 * «Local API ● 127.0.0.1:45678» или «выключен». Состояние называется и текстом для экранного диктора (`spoken`),
 * а не только цветом точки; причина отказа — переводом кода, в подсказке и для диктора.
 */
const apiLine = computed(() => {
  const status = settings.apiStatus
  if (status.state === 'off') return { state: status.state, text: t('nav.apiOff'), spoken: null, title: undefined }
  const text = `127.0.0.1:${status.port}`
  if (status.state === 'listening') {
    return { state: status.state, text, spoken: t('nav.apiListening'), title: undefined }
  }
  const reason = apiFailureText(status.code)
  return { state: status.state, text, spoken: t('nav.apiNotStarted', { reason }), title: t('nav.apiFailed', { reason }) }
})

const sections = computed<{ label: string; items: NavItem[] }[]>(() => [
  { label: t('nav.workspace'), items: [
    { to: '/profiles', label: t('nav.profiles'), icon: 'profiles' },
    { to: '/proxies', label: t('nav.proxies'), icon: 'proxies' },
    { to: '/groups', label: t('nav.groups'), icon: 'groups' },
  ] },
  { label: t('nav.tools'), items: [
    { to: '/extensions', label: t('nav.extensions'), icon: 'extensions' },
    { to: '/automation', label: t('nav.automation'), icon: 'automation' },
    { to: '/sync', label: t('nav.sync'), icon: 'sync' },
  ] },
])
const isActive = (to: string) => route.path.startsWith(to)
const engineLine = computed(() => {
  const major = engineMajor(app.engineVersion)
  return major ? t('nav.engine', { version: major }) : t('nav.engineMissing')
})
</script>

<template>
  <aside :class="['sidebar', { collapsed }]">
    <div class="brand-row">
      <OmbraMark :compact="collapsed" />
      <UiTooltip :text="t(collapsed ? 'nav.expand' : 'nav.collapse')"><button
        type="button"
        class="collapse-toggle"
        :aria-label="t(collapsed ? 'nav.expand' : 'nav.collapse')"
        :title="t(collapsed ? 'nav.expand' : 'nav.collapse')"
        :aria-expanded="!collapsed"
        aria-controls="sidebar-navigation"
        @click="collapsed = !collapsed"
      >
        <AppIcon :name="collapsed ? 'panelExpand' : 'panelCollapse'" :size="20" />
      </button></UiTooltip>
    </div>
    <nav id="sidebar-navigation" :aria-label="t('nav.sections')" class="nav">
      <section v-for="section in sections" :key="section.label" class="nav-section" :aria-label="section.label">
        <div :class="['category', { 'sr-only': collapsed }]">{{ section.label }}</div>
        <UiTooltip v-for="item in section.items" :key="item.icon" :text="collapsed ? item.label : undefined"><RouterLink
          :to="item.to"
          :class="['item', { active: isActive(item.to) }]"
          :aria-current="isActive(item.to) ? 'page' : undefined"
          :aria-label="item.label"
          :title="collapsed ? item.label : undefined"
        >
          <AppIcon :name="item.icon" :size="18" />
          <span v-if="!collapsed" class="item-label">{{ item.label }}</span>
          <span v-if="!collapsed && item.to === '/profiles' && groups.loaded" class="nav-count">{{ groups.rows.length }}</span>
        </RouterLink></UiTooltip>
      </section>
      <section v-if="!collapsed && groups.quickAccess.length && route.path !== '/profiles'" class="nav-section folders" :aria-label="t('nav.folders')">
        <div :class="['category', { 'sr-only': collapsed }]">{{ t('nav.folders') }}</div>
        <button
          v-for="group in groups.quickAccess"
          :key="group.id"
          type="button"
          :class="['item', { active: route.path === '/profiles' && profiles.filters.group === group.id }]"
          :aria-label="group.name"
          :aria-pressed="route.path === '/profiles' && profiles.filters.group === group.id"
          :title="group.name"
          @click="openGroup(group.id)"
        >
          <AppIcon name="folder" :size="18" />
          <span v-if="!collapsed" class="item-label">{{ group.name }}</span>
          <span v-if="!collapsed" class="nav-count">{{ group.profile_count }}</span>
        </button>
      </section>

    </nav>
    <div class="spacer" />
    <div class="bottom">
      <UiTooltip :text="collapsed ? t('nav.trash') : undefined"><RouterLink to="/trash" :class="['item', { active: isActive('/trash') }]" :aria-current="isActive('/trash') ? 'page' : undefined" :aria-label="t('nav.trash')" :title="collapsed ? t('nav.trash') : undefined">
          <AppIcon name="trash" :size="18" /><span v-if="!collapsed">{{ t('nav.trash') }}</span>
        </RouterLink></UiTooltip>
      <UiTooltip :text="collapsed ? t('nav.settings') : undefined">
      <RouterLink
        to="/settings"
        :class="['item', { active: isActive('/settings') }]"
        :aria-current="isActive('/settings') ? 'page' : undefined"
        :aria-label="t('nav.settings')"
        :title="collapsed ? t('nav.settings') : undefined"
      >
        <AppIcon name="settings" :size="18" /><span v-if="!collapsed">{{ t('nav.settings') }}</span>
      </RouterLink></UiTooltip>
    </div>
    <div class="footer">
      <div v-if="!collapsed">{{ engineLine }}</div>
      <div v-if="settings.loaded" class="api-line" :title="collapsed ? [t('nav.api'), apiLine.text, apiLine.spoken].filter(Boolean).join(' · ') : apiLine.title">
        <span :class="['api-dot', apiLine.state]" aria-hidden="true" />
        <span :class="{ 'sr-only': collapsed }">{{ t('nav.api') }}</span>
        <span :class="{ mono: apiLine.state !== 'off', 'sr-only': collapsed }">{{ apiLine.text }}</span>
        <span v-if="apiLine.spoken" class="sr-only">{{ apiLine.spoken }}</span>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  width: 248px;
  min-height: 0;
  padding: 16px 12px;
  border-right: 1px solid var(--border);
  background: var(--sidebar);
  color: var(--sidebar-strong);
}

.sidebar.collapsed {
  width: 72px;
  padding-inline: 10px;
}

.brand-row {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-height: 44px;
  margin-bottom: 16px;
}

.collapse-toggle {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  width: 44px;
  height: 44px;
  padding: 0;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--text-2);
  cursor: pointer;
}

.collapse-toggle:hover {
  background: var(--sidebar-active);
  color: var(--accent-ink);
}

.collapse-toggle:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}

.collapsed .brand-row {
  flex-direction: column;
  gap: 12px;
  margin-bottom: 12px;
}

.nav {
  overflow-y: auto;
  overflow-x: hidden;
  flex-shrink: 1;
  scrollbar-width: thin;
}

.nav-section {
  padding: 12px 0;
  border-top: 1px solid var(--border);
}

.nav-section:first-child {
  padding-top: 0;
  border-top: 0;
}

.category {
  padding: 0 12px 8px;
  font-size: 12px;
  line-height: 18px;
  font-weight: 500;
  color: var(--sidebar-muted);
}

.item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  min-height: 44px;
  padding: 8px 12px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--sidebar-text);
  font-weight: 400;
  text-align: left;
  text-decoration: none;
  cursor: pointer;
}

.item + .item {
  margin-top: 2px;
}

.item-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nav-count {
  margin-left: auto;
  padding-left: 2px;
  font-size: 12px;
  color: var(--sidebar-muted);
}

.item:hover {
  background: var(--sidebar-hover);
  color: var(--text);
}

.item:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: -2px;
}

.item.active {
  background: var(--sidebar-active);
  color: var(--accent-ink);
  font-weight: 500;
}

.item.active::before {
  content: '';
  position: absolute;
  top: 6px;
  bottom: 6px;
  left: -3px;
  width: 3px;
  border-radius: 3px;
  background: var(--accent);
}

.item.active .nav-count {
  color: var(--accent-ink);
}

.collapsed .item {
  justify-content: center;
  padding-inline: 0;
}

.spacer {
  flex-grow: 1;
  min-height: 20px;
}

.bottom {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}

.footer {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 12px;
  padding: 12px 8px 0;
  border-top: 1px solid var(--border);
  font-size: 11px;
  line-height: 16px;
  color: var(--sidebar-muted);
}

.collapsed .footer {
  align-items: center;
}

.api-line {
  display: flex;
  align-items: center;
  gap: 6px;
}

.api-dot {
  flex-shrink: 0;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--sidebar-muted);
}

.api-dot.listening {
  background: var(--ok);
}

.api-dot.failed {
  background: var(--danger-dot);
}

</style>
