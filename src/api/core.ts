import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { toCoreError } from './errors'
import type {
  Announcement,
  SecurityStatus,
  SecuritySetup,
  BatchSummary,
  FlowDoc,
  FlowRow,
  OpsCatalog,
  Report,
  JournalEntry,
  RunRow,
  TablePreview,
  TablePreviewForm,
  TaskForm,
  TaskSettingsForm,
  TaskDetails,
  TaskUpdateForm,
  TableDataPreviewForm,
  TaskRow,
  DebugEvent,
  TasksChanged,
  SyncSettings,
  SyncState,
  ApiConfig,
  ApiConfigForm,
  ApiStatus,
  AppInfo,
  BulkCreateForm,
  BulkEditForm,
  BulkReport,
  ConnRecord,
  ExtCategory,
  ExtensionRecord,
  CookieFormat,
  CsvImportReport,
  CookieImportReport,
  CookieSource,
  CookieStats,
  EngineBuild,
  EngineCatalog,
  EngineProgress,
  EngineUpdateOutcome,
  ExportTarget,
  FieldError,
  Fingerprint,
  FingerprintOptions,
  GeoInfo,
  GroupForm,
  GroupList,
  GroupRecord,
  Language,
  ListQuery,
  ProfileDetails,
  ProfileForm,
  ProfileList,
  ProfileRecord,
  ProfileRow,
  ProfileLabel,
  ProfileSettings,
  ProxyEditForm,
  ProxyImportReport,
  ProxyKind,
  ProxyRecord,
  ProxyRow,
  QuitAction,
  RunningInfo,
  SupervisorEvent,
  TagRecord,
  TrafficSummary,
  TotpCode,
} from './types'

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args)
  } catch (error) {
    throw toCoreError(error)
  }
}

/** Команды ядра (`src-tauri/src/commands.rs`). */
export const core = {
  securityStatus: () => call<SecurityStatus>('security_status'),
  securityBeginSetup: (password: string) => call<SecuritySetup>('security_begin_setup', { password }),
  securityVerifySetup: (code: string) => call<null>('security_verify_setup', { code }),
  securityCancelSetup: () => call<null>('security_cancel_setup'),
  securityFinishSetup: (recoverySaved: boolean) => call<null>('security_finish_setup', { recoverySaved }),
  securityUnlock: (password: string, code: string, recovery: boolean) => call<null>('security_unlock', { password, code, recovery }),
  securityChangePassword: (current: string, code: string, newPassword: string) => call<string[]>('security_change_password', { current, code, newPassword }),
  securityReplaceAuthenticator: (password: string, code: string) => call<SecuritySetup>('security_replace_authenticator', { password, code }),
  listAnnouncements: () => call<Announcement[]>('list_announcements'),
  openAnnouncement: (id: string) => call<null>('open_announcement', { id }),
  listProfileLabels: () => call<ProfileLabel[]>('list_profile_labels'),
  saveProfileLabel: (id: number | null, name: string, color: string) => call<ProfileLabel>('save_profile_label', { id, name, color }),
  deleteProfileLabel: (id: number) => call<null>('delete_profile_label', { id }),
  setProfileLabel: (id: string, labelId: number | null) => call<ProfileLabel | null>('set_profile_label', { id, labelId }),
  appInfo: () => call<AppInfo>('app_info'),
  listProfiles: (query: ListQuery) => call<ProfileList>('list_profiles', { query }),
  getProfile: (id: string) => call<ProfileDetails>('get_profile', { id }),
  resetPersona: (id: string) => call<ProfileDetails>('reset_persona', { id }),
  totpCode: (id: string) => call<TotpCode>('totp_code', { id }),
  createProfile: (form: ProfileForm) => call<ProfileRecord>('create_profile', { form }),
  updateProfile: (id: string, form: ProfileForm) => call<ProfileRecord>('update_profile', { id, form }),
  deleteProfile: (id: string) => call<null>('delete_profile', { id }),
  startProfile: (id: string) => call<RunningInfo>('start_profile', { id }),
  stopProfile: (id: string) => call<null>('stop_profile', { id }),
  newFingerprint: () => call<Fingerprint>('new_fingerprint'),
  regenerateFingerprint: (current: Fingerprint) => call<Fingerprint>('regenerate_fingerprint', { current }),
  validateFingerprint: (fingerprint: Fingerprint) => call<FieldError[]>('validate_fingerprint', { fingerprint }),
  fingerprintOptions: () => call<FingerprintOptions>('fingerprint_options'),
  /** Распаковка архива в новую папку версии; предыдущие браузеры сохраняются. */
  importEngine: (path: string) => call<EngineBuild>('import_engine', { path }),
  engineCatalog: () => call<EngineCatalog>('engine_catalog'),
  selectEngine: (id: string) => call<EngineBuild>('select_engine', { id }),
  setEngineAutomatic: (automatic: boolean) => call<null>('set_engine_automatic', { automatic }),
  updateEngine: (background = false) => call<EngineUpdateOutcome>('update_engine', { background }),
  cancelEngineUpdate: () => call<null>('cancel_engine_update'),
  setLanguage: (language: Language) => call<null>('set_language', { language }),
  revealDataDir: () => call<null>('reveal_data_dir'),
  confirmQuit: () => call<null>('confirm_quit'),
  /** Интерфейс подписался на события: ядро задаёт отложенный вопрос о выходе. */
  uiReady: () => call<null>('ui_ready'),
  listGroups: () => call<GroupList>('list_groups'),
  createGroup: (form: GroupForm) => call<GroupRecord>('create_group', { form }),
  updateGroup: (id: number, form: GroupForm) => call<GroupRecord>('update_group', { id, form }),
  deleteGroup: (id: number) => call<null>('delete_group', { id }),
  /** «Запустить все» у группы; `null` — «Без группы». */
  startGroup: (group: number | null) => call<BulkReport>('start_group', { group }),
  listTags: () => call<TagRecord[]>('list_tags'),
  changeProfileTags: (id: string, add: string[], remove: string[]) => call<string[]>('change_profile_tags', { id, add, remove }),
  cloneProfiles: (ids: string[]) => call<BulkReport>('clone_profiles', { ids }),
  createProfilesBulk: (form: BulkCreateForm) => call<ProfileRecord[]>('create_profiles_bulk', { form }),
  startProfiles: (ids: string[]) => call<BulkReport>('start_profiles', { ids }),
  arrangeWindows: () => call<BulkReport>('arrange_windows'),
  syncState: () => call<SyncState>('sync_state'),
  syncStart: (main: string, followers: string[]) => call<SyncState>('sync_start', { main, followers }),
  syncStop: () => call<null>('sync_stop'),
  /** «Отпустить» ведомое окно: пауза, окно можно поправить руками. */
  syncRelease: (id: string) => call<null>('sync_release', { id }),
  /** «Вернуть» ведомое окно в сессию. */
  syncResume: (id: string) => call<null>('sync_resume', { id }),
  /** Окно открывает адрес видимой вкладки главного. */
  syncOpenLikeMain: (id: string) => call<null>('sync_open_like_main', { id }),
  /** Окно профиля выходит на передний план. */
  showWindow: (id: string) => call<null>('show_window', { id }),
  getSyncSettings: () => call<SyncSettings>('get_sync_settings'),
  setSyncSettings: (settings: SyncSettings) => call<null>('set_sync_settings', { settings }),
  trafficSummary: (id: string) => call<TrafficSummary>('traffic_summary', { id }),
  connectionLog: (id: string, limit: number) => call<ConnRecord[]>('connection_log', { id, limit }),
  listExtensions: () => call<ExtensionRecord[]>('list_extensions'),
  /** Идентификатор или ссылка Chrome Web Store. */
  addStoreExtension: (input: string) => call<ExtensionRecord>('add_store_extension', { input }),
  /** Поиск Chrome Web Store в системном браузере; пустой запрос — главная магазина. */
  openStoreSearch: (query: string) => call<null>('open_store_search', { query }),
  /** Страница расширения в Chrome Web Store в системном браузере. */
  openStorePage: (id: string) => call<null>('open_store_page', { id }),
  /** `.crx`, `.zip` или папка с `manifest.json`. */
  addFileExtension: (path: string) => call<ExtensionRecord>('add_file_extension', { path }),
  updateExtension: (id: string) => call<ExtensionRecord>('update_extension', { id }),
  removeExtension: (id: string) => call<null>('remove_extension', { id }),
  listExtCategories: () => call<ExtCategory[]>('list_ext_categories'),
  createExtCategory: (name: string, remark: string) => call<ExtCategory>('create_ext_category', { name, remark }),
  updateExtCategory: (id: number, name: string, remark: string) =>
    call<ExtCategory>('update_ext_category', { id, name, remark }),
  setExtCategoryItems: (id: number, extensionIds: string[]) =>
    call<ExtCategory>('set_ext_category_items', { id, extensionIds }),
  deleteExtCategory: (id: number) => call<null>('delete_ext_category', { id }),
  setProfilesExtCategory: (ids: string[], categoryId: number) =>
    call<BulkReport>('set_profiles_ext_category', { ids, categoryId }),
  /** «Установить в группу»; `null` — «Без группы». */
  setGroupExtCategory: (groupId: number | null, categoryId: number) =>
    call<BulkReport>('set_group_ext_category', { groupId, categoryId }),
  stopProfiles: (ids: string[]) => call<BulkReport>('stop_profiles', { ids }),
  deleteProfiles: (ids: string[]) => call<BulkReport>('delete_profiles', { ids }),
  listTrash: () => call<ProfileRow[]>('list_trash'),
  restoreProfiles: (ids: string[]) => call<BulkReport>('restore_profiles', { ids }),
  purgeProfiles: (ids: string[]) => call<BulkReport>('purge_profiles', { ids }),
  emptyTrash: () => call<BulkReport>('empty_trash'),
  moveProfiles: (ids: string[], group: number | null) => call<BulkReport>('move_profiles', { ids, group }),
  editProfiles: (ids: string[], form: BulkEditForm) => call<BulkReport>('edit_profiles', { ids, form }),
  exportCookiesTo: (ids: string[], format: CookieFormat, target: ExportTarget) =>
    call<BulkReport>('export_cookies_to', { ids, format, target }),
  exportProfile: (id: string, path: string, password: string | null) => call<null>('export_profile', { id, path, password }),
  importProfile: (path: string, password: string | null) => call<ProfileRecord>('import_profile', { path, password }),
  /** Профили по фильтрам — в файл CSV; ответ — число профилей в файле. */
  exportCsvFile: (query: ListQuery, withSecrets: boolean, path: string) =>
    call<number>('export_csv_file', { query, withSecrets, path }),
  /** Профили из файла CSV (до 10 МБ, UTF-8). */
  importCsvFile: (path: string) => call<CsvImportReport>('import_csv_file', { path }),
  listProxies: () => call<ProxyRow[]>('list_proxies'),
  createProxy: (form: ProxyEditForm) => call<ProxyRecord>('create_proxy', { form }),
  updateProxy: (id: number, form: ProxyEditForm) => call<ProxyRecord>('update_proxy', { id, form }),
  opsCatalog: () => call<OpsCatalog>('ops_catalog'),
  getFlow: (id: number) => call<FlowDoc>('get_flow', { id }),
  validateFlow: (flow: FlowDoc) => call<Report>('validate_flow', { flow }),
  saveFlow: (id: number | null, flow: FlowDoc) => call<FlowRow>('save_flow', { id, flow }),
  listFlows: () => call<FlowRow[]>('list_flows'),
  importFlowJson: (source: string) => call<FlowRow>('import_flow_json', { source }),
  importFlowFile: (path: string) => call<FlowRow>('import_flow_file', { path }),
  exportFlowFile: (id: number, path: string) => call<null>('export_flow_file', { id, path }),
  copyFlow: (id: number) => call<FlowRow>('copy_flow', { id }),
  deleteFlow: (id: number) => call<null>('delete_flow', { id }),
  setFlowDefaults: (id: number, values: Record<string, unknown>) => call<FlowRow>('set_flow_defaults', { id, values }),
  listTasks: () => call<TaskRow[]>('list_tasks'),
  getTask: (taskId: number) => call<TaskDetails>('get_task', { taskId }),
  updateTask: (taskId: number, form: TaskUpdateForm) => call<null>('update_task', { taskId, form }),
  previewTableData: (form: TableDataPreviewForm) => call<TablePreview>('preview_table_data', { form }),
  listBatches: (taskId: number) => call<BatchSummary[]>('list_batches', { taskId }),
  listRuns: (batchId: number) => call<RunRow[]>('list_runs', { batchId }),
  /** Отладка процесса в окне профиля `serial`; `from` — путь групп и узел «С этого узла». Возвращает номер запуска. */
  debugFlow: (flow: FlowDoc, serial: number, from: string[] | null) => call<string>('debug_flow', { flow, serial, from }),
  /** «Проверить» в форме узла: значение, которое дал бы узел `nodeId` (без страницы, переменные — умолчания Start). */
  testNode: (flow: FlowDoc, nodeId: string) => call<string>('test_node', { flow, nodeId }),
  stopDebug: (runId: string) => call<boolean>('stop_debug', { runId }),
  runJournal: (runId: number) => call<JournalEntry[]>('run_journal', { runId }),
  deleteTask: (taskId: number) => call<null>('delete_task', { taskId }),
  openBatchFiles: (batchId: number) => call<null>('open_batch_files', { batchId }),
  createTask: (form: TaskForm) => call<number>('create_task', { form }),
  updateTaskSettings: (taskId: number, form: TaskSettingsForm) => call<null>('update_task_settings', { taskId, form }),
  startTask: (taskId: number, failed = false) => call<null>('start_task', { taskId, failed }),
  stopTask: (taskId: number) => call<boolean>('stop_task', { taskId }),
  cancelWaiting: (taskId: number) => call<boolean>('cancel_waiting', { taskId }),
  previewTable: (form: TablePreviewForm) => call<TablePreview>('preview_table', { form }),
  deleteProxy: (id: number) => call<null>('delete_proxy', { id }),
  setProxyEnabled: (id: number, enabled: boolean) => call<null>('set_proxy_enabled', { id, enabled }),
  /** `kind` — тип для строк без схемы («Тип, если не указан»). */
  importProxies: (text: string, kind: ProxyKind) => call<ProxyImportReport>('import_proxies', { text, kind }),
  checkProxy: (id: number) => call<GeoInfo>('check_proxy', { id }),
  checkConnection: (target: import('./types').ConnectionCheck, checker: import('./types').IpChecker) => call<GeoInfo>('check_connection', { target, checker }),
  cookieStats: (id: string) => call<CookieStats>('cookie_stats', { id }),
  importCookies: (id: string, source: CookieSource) => call<CookieImportReport>('import_cookies', { id, source }),
  clearCookies: (id: string) => call<null>('clear_cookies', { id }),
  profileSettings: () => call<ProfileSettings>('profile_settings'),
  setOnQuit: (action: QuitAction) => call<null>('set_on_quit', { action }),
  getServicesProxy: () => call<number | null>('get_services_proxy'),
  setServicesProxy: (id: number | null) => call<null>('set_services_proxy', { id }),
  setGoogleKey: (path: string) => call<string>('set_google_key', { path }),
  clearGoogleKey: () => call<null>('clear_google_key'),
  googleKeyEmail: () => call<string | null>('google_key_email'),
  apiConfig: () => call<ApiConfig>('api_config'),
  setApiConfig: (form: ApiConfigForm) => call<ApiConfig>('set_api_config', { form }),
  regenerateApiKey: () => call<ApiConfig>('regenerate_api_key'),
  clearCache: (id: string) => call<null>('clear_cache', { id }),
}

/** События ядра. */
export const events = {
  onEngineProgress: (handler: (progress: EngineProgress) => void): Promise<UnlistenFn> =>
    listen<EngineProgress>('engine-progress', (event) => handler(event.payload)),
  /** Local API изменил профили, группы или прокси: списки перезапрашиваются. */
  onProfilesChanged: (handler: () => void): Promise<UnlistenFn> => listen('profiles-changed', () => handler()),
  onSupervisor: (handler: (event: SupervisorEvent) => void): Promise<UnlistenFn> =>
    listen<SupervisorEvent>('supervisor', (event) => handler(event.payload)),
  onQuitRequested: (handler: (active: number) => void): Promise<UnlistenFn> =>
    listen<number>('quit-requested', (event) => handler(event.payload)),
  /** Пакет автоматизации начался, шёл или закончился: хранилище обновляет список задач. */
  onTasksChanged: (handler: (event: TasksChanged) => void): Promise<UnlistenFn> =>
    listen<TasksChanged>('tasks-changed', (event) => handler(event.payload)),
  /** Сессия синхронизатора изменилась. */
  onSync: (handler: (state: SyncState) => void): Promise<UnlistenFn> =>
    listen<SyncState>('sync', (event) => handler(event.payload)),
  /** Ход отладки процесса: узел начат, строка журнала, конец. */
  onFlowDebug: (handler: (event: DebugEvent) => void): Promise<UnlistenFn> =>
    listen<DebugEvent>('flow-debug', (event) => handler(event.payload)),
  /** Состояние сервера Local API: после запуска приложения и каждой смены настроек. */
  onApiStatus: (handler: (status: ApiStatus) => void): Promise<UnlistenFn> =>
    listen<ApiStatus>('api-status', (event) => handler(event.payload)),
}
