import { createRouter, createWebHashHistory, type RouteLocationNormalizedLoaded } from 'vue-router'
import ExtensionsView from './views/ExtensionsView.vue'
import GroupsView from './views/GroupsView.vue'
import ProfileEditorView from './views/ProfileEditorView.vue'
import ProfilesView from './views/ProfilesView.vue'
import ProxiesView from './views/ProxiesView.vue'
import SettingsView from './views/SettingsView.vue'
import SyncSettingsView from './views/SyncSettingsView.vue'
import FlowEditorView from './views/automation/editor/FlowEditorView.vue'
import FlowsView from './views/automation/FlowsView.vue'
import FlowLibraryView from './views/automation/FlowLibraryView.vue'
import JournalView from './views/automation/JournalView.vue'
import TasksView from './views/automation/TasksView.vue'
import SyncView from './views/SyncView.vue'
import TrashView from './views/TrashView.vue'

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', redirect: '/profiles' },
    { path: '/profiles', component: ProfilesView },
    { path: '/profiles/new', component: ProfileEditorView, meta: { focusedLayout: true } },
    { path: '/profiles/:id', component: ProfileEditorView, meta: { focusedLayout: true } },
    { path: '/groups', component: GroupsView },
    { path: '/proxies', component: ProxiesView },
    { path: '/extensions', component: ExtensionsView },
    { path: '/extensions/sets', component: ExtensionsView },
    { path: '/automation', redirect: '/automation/flows' },
    { path: '/automation/flows', component: FlowsView },
    { path: '/automation/library/:templateId?', component: FlowLibraryView },
    { path: '/automation/flows/new', component: FlowEditorView },
    { path: '/automation/flows/:id', component: FlowEditorView },
    { path: '/automation/tasks', component: TasksView },
    { path: '/automation/journal', component: JournalView },
    { path: '/sync', component: SyncView },
    { path: '/sync/settings', component: SyncSettingsView },
    { path: '/trash', component: TrashView },
    { path: '/settings', component: SettingsView },
  ],
})

/**
 * Ключ `RouterView`: экран пересоздаётся при каждом переходе, кроме редактора процесса — он живёт между своими адресами:
 * первое сохранение меняет адрес new → id без перезагрузки, другой процесс редактор загружает сам.
 */
export const viewKey = (route: RouteLocationNormalizedLoaded) => {
  if (route.path.startsWith('/automation/flows/')) return 'flow-editor'
  // Поиск и переход в карточку сохраняют каталог и его фокус.
  if (route.path === '/automation/library' || route.path.startsWith('/automation/library/')) return 'flow-library'
  // Разделы «Настроек» — якоря одного экрана: смена якоря его не пересоздаёт.
  return route.path === '/settings' ? route.path : route.fullPath
}
