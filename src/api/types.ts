/** Ответы и формы ядра (Rust: `src-tauri/src/app`, `supervisor`, `store`) в JSON. */

export type Language = 'ru' | 'en'
export type ProxyKind = 'http' | 'https' | 'socks5'
export type FieldName =
  | 'os_version'
  | 'gpu'
  | 'hardware_concurrency'
  | 'device_memory'
  | 'screen'
  | 'timezone'
  | 'language'
  | 'webrtc'
  | 'noise'
  | 'storage_quota_mb'
  | 'device_pixel_ratio'
  | 'max_touch_points'
  | 'location'

export type Setting = { mode: 'auto' } | { mode: 'manual'; value: string }
export type WebRtc = { mode: 'auto' } | { mode: 'manual'; ip: string }

export type Location = { mode: 'off' } | { mode: 'manual'; lat: number; lon: number }

export interface Gpu {
  vendor: string
  renderer: string
}

export interface Screen {
  width: number
  height: number
}

export interface Fingerprint {
  seed: number
  os: 'macos' | 'windows'
  os_version: string
  brand: string
  gpu: Gpu
  hardware_concurrency: number
  device_memory: number
  screen: Screen
  timezone: Setting
  language: Setting
  webrtc: WebRtc
  noise: boolean
  storage_quota_mb: number | null
  device_pixel_ratio: number
  max_touch_points: number
  location: Location
  manual: FieldName[]
}

export interface FieldError {
  field: string
  code: string
}

export interface CoreErrorPayload {
  code: string
  message: string
  fields?: FieldError[]
  ownerHint?: string
}

export interface RunningInfo {
  pid: number
  cdp_ws: string
  debug_port: number
  relay_port: number
  proxy_id: number
  exit_ip: string | null
  country: string | null
  timezone: string
  started_at: number
}

export type ProfileStatus =
  | { state: 'stopped' }
  | { state: 'starting' }
  | ({ state: 'running' } & RunningInfo)
  | ({ state: 'paused' } & RunningInfo)
  /** Ждёт поток лицензии: `info` есть — браузер работает, но потерял поток; `null` — запуск ещё идёт. */
  | { state: 'waiting'; license: string; info: RunningInfo | null; ownerHint?: string }
  | { state: 'crashed'; code: number | null; signal: number | null; at: number }

export type BrowserStateFilter = ProfileStatus['state']
export type ProfileLabelFilter = 'any' | 'none' | number
export type SortKey = 'serial' | 'name' | 'last_opened' | 'created'

export interface ListQuery {
  search: string
  sort: SortKey
  descending: boolean
  /** Техническое состояние браузера; ручной статус профиля — `label_id`. */
  status: BrowserStateFilter | null
  /** Ручной статус: null/отсутствует — любой, 0 — без статуса, положительное число — id метки. */
  label_id?: number | null
  group_id: number | null
  /** «Без группы»; важнее `group_id`. */
  ungrouped: boolean
  tag: string | null
}

export interface GatewayView {
  kind: ProxyKind
  host: string
  port: number
  ip: string | null
  country: string | null
  /** `null` — прокси ещё не проверяли. */
  ok: boolean | null
}

export interface GroupView {
  id: number
  name: string
  color: string
}

export interface ProfileRow {
  direct?: boolean
  id: string
  serial: number
  name: string
  note: string
  group: GroupView | null
  tags: string[]
  label: ProfileLabel | null
  gateway: GatewayView | null
  /** Шлюз заменён при последнем запуске. */
  gateway_replaced_at: number | null
  restore_tabs: boolean
  os_version: string
  brand: string
  seed: number
  status: ProfileStatus
  created_at: number
  last_opened_at: number | null
  /** В корзине с этого времени (unix); null — живой профиль. */
  deleted_at: number | null
  /** Набор расширений: раздел «Расширения» в панели группы. */
  ext_category_id: number
}

/** Ручной статус профиля; состояние запущенного браузера хранится отдельно в `status`. */
export interface ProfileLabel {
  id: number
  name: string
  color: string
  preset: 'ban' | 'ready' | 'new' | null
}

export interface ProfileList {
  rows: ProfileRow[]
  total: number
  running: number
}

export type HumanizePreset = 'default' | 'careful' | 'fast'

/** Очеловечивание ввода профиля (этап Э5 движка): флаги `--fingerprint-humanize*` при запуске. */
export interface Humanize {
  enabled: boolean
  preset: HumanizePreset
  /** Блокировать мышь и клавиатуру человека во вкладке, которой управляет скрипт. */
  block_input: boolean
  /** Горячая клавиша «Вставить как человек»: `Ctrl+Alt+V` — ⌃ и/или ⌥, ⌘ нельзя; пусто — нет. */
  paste_key: string
}

/** Почерк профиля — персона движка, выведенная из seed. */
export interface PersonaView {
  generator_version: number
  keys_per_sec: number
  typo_rate: number
  dwell_ms: number
  mouse_speed: number
  scroll_device: 'trackpad' | 'wheel'
}

export interface ProfileRecord {
  network?: ProfileNetwork
  id: string
  serial: number
  name: string
  note: string
  group_id: number | null
  platform_domain: string
  start_urls: string[]
  extra_args: string[]
  username: string | null
  has_password: boolean
  has_totp: boolean
  fingerprint_json: string
  proxy_id: number | null
  created_at: number
  last_opened_at: number | null
  restore_tabs: boolean
  gateway_replaced_at: number | null
  /** В корзине с этого времени (unix); null — живой профиль. */
  deleted_at: number | null
  /** Набор расширений; 0 — «Общий». */
  ext_category_id: number
  humanize: Humanize
  /** Seed персоны движка — строкой: в числе JavaScript 64 бита теряются. */
  humanize_seed: string
}

export interface ProxyCheck {
  at: number
  ok: boolean
  exit_ip: string | null
  country: string | null
}

export interface ProxyRecord {
  id: number
  kind: ProxyKind
  host: string
  port: number
  username: string | null
  has_password: boolean
  note: string
  enabled: boolean
  /** `null`, пока прокси ни разу не проверяли. */
  check: ProxyCheck | null
}

export interface Bookmark {
  title: string
  url: string
}

export interface ProfileDetails {
  profile: ProfileRecord
  fingerprint: Fingerprint
  proxy: ProxyRecord | null
  tags: string[]
  bookmarks: Bookmark[]
  persona: PersonaView
}

export type SecretInput = { action: 'keep' } | { action: 'clear' } | { action: 'set'; value: string }

export interface ProfileForm {
  network?: ProfileNetwork
  inline_proxy?: ProxyEditForm
  inline_proxy_source?: number | null
  name: string
  note: string
  platform_domain: string
  start_urls: string[]
  extra_args: string[]
  username: string
  password: SecretInput
  fingerprint: Fingerprint
  /** Нет поля — шлюз не меняется; `null` — снять закрепление (при первом запуске выберется случайный); число — закрепить. */
  proxy_id?: number | null
  /** Нет поля — группа не меняется; `null` — «Без группы». */
  group_id?: number | null
  /** Нет поля — не меняется (при создании «да»). */
  restore_tabs?: boolean
  /** Ключ 2FA (Base32). Нет поля — не меняется. */
  totp?: SecretInput
  /** Нет поля — теги не меняются; список заменяет набор тегов профиля. */
  tags?: string[]
  /** Нет поля — закладки не меняются; список заменяет прежний. */
  bookmarks?: Bookmark[]
  /** Набор расширений. Нет поля — не меняется (при создании «Общий», 0). */
  ext_category_id?: number
  /** Очеловечивание ввода. Нет поля — не меняется (при создании включено). */
  humanize?: Humanize
}

export interface TotpCode {
  code: string
  seconds_left: number
}

export interface GeoInfo {
  ip: string
  country: string
  timezone: string
}

export type IpChecker = 'auto' | 'ipwhois' | 'ipapi' | 'ip2location' | 'ip-api'
export interface ProfileNetwork { direct: boolean; ip_checker: IpChecker }
export type ConnectionCheck = { mode: 'direct' } | { mode: 'saved'; id: number } | { mode: 'manual'; proxy: ProxyEditForm; source_id: number | null }

export interface FingerprintOptions {
  screens: Screen[]
  gpus: Gpu[]
  hardware_concurrency: number[]
  device_memory: number[]
  engine_version: string | null
}

export interface EngineBuild {
  version: string
  binary: string
}

export interface InstalledEngine {
  id: string
  version: string
  chromium: string
  binary: string
  active: boolean
}

export interface EngineCatalog {
  versions: InstalledEngine[]
  automatic: boolean
}

export interface EngineProgress {
  phase: 'checking' | 'downloading' | 'verifying' | 'installing' | 'current' | 'complete'
  version: string | null
  downloaded: number
  total: number | null
}

export interface EngineUpdateOutcome {
  state: 'updated' | 'current' | 'pinned'
  catalog: EngineCatalog
}

export interface AppInfo {
  version: string
  data_dir: string
  language: Language
  engine: EngineBuild | null
  init_error: CoreErrorPayload | null
}

export interface SecurityStatus {
  supported?: boolean
  configured: boolean
  unlocked: boolean
  migrating: boolean
  recovery_remaining: number
  retry_after: number
  recovery_recent?: boolean
}
export interface SecuritySetup {
  secret: string
  uri: string
  recovery_codes: string[]
}
export interface Announcement {
  id: string
  revision: string
  title: { ru: string; en: string }
  body: { ru: string; en: string }
  action_label?: { ru: string; en: string } | null
  url?: string | null
  tone: 'info' | 'promotion'
  dismissible: boolean
}

export type SupervisorEvent =
  | { kind: 'status'; profile_id: string; status: ProfileStatus }
  | { kind: 'gateway_unreachable'; profile_id: string }
  | { kind: 'gateway_replaced'; profile_id: string; name: string }

export interface GroupRecord {
  id: number
  name: string
  color: string
  note: string
  quick_access: boolean
  created_at: number
  profile_count: number
}

export interface GroupList {
  groups: GroupRecord[]
  /** Сколько профилей «Без группы». */
  ungrouped: number
}

export interface GroupForm {
  name: string
  color: string
  note: string
  quick_access: boolean
}

export interface TagRecord {
  id: number
  name: string
  profile_count: number
}

/** Строка экрана «Прокси»: запись, сколько профилей за ней закреплено и кто её сейчас занимает. */
export interface ProxyRow extends ProxyRecord {
  profiles: number
  /** id запущенного профиля, который держит прокси. */
  used_by: string | null
  /** Имя этого профиля: «работает: <имя>», «занят: <имя>». */
  used_by_name: string | null
  /** Имя профиля, если прокси закреплён ровно за одним. */
  pinned_to: string | null
}

export interface BulkFailure {
  id: string
  error: CoreErrorPayload
}

/** Расширение библиотеки (спецификация 3Б). */
export interface ExtensionRecord {
  /** Идентификатор Chromium: 32 буквы a–p. */
  id: string
  name: string
  version: string
  source: 'store' | 'file'
  manifest_version: number
  /** Право `proxy` или `privacy`: расширение может пустить трафик мимо шлюза. */
  gateway_risk: boolean
  added_at: number
  updated_at: number
  /** Поле `description` манифеста; может быть пустым. */
  description: string
  /** Имя файла или папки, из которых добавлено; у расширений из магазина — null. */
  file_name: string | null
}

/** Набор расширений — категория AdsPower; `id` 0 — встроенный «Общий». */
export interface ExtCategory {
  id: number
  name: string
  remark: string
  extension_ids: string[]
  /** Живые профили с этим набором. */
  profile_count: number
}

/** Сводка трафика профиля; `live` — профиль работает, в суммах живая статистика. */
export interface TrafficSummary {
  bytes_up: number
  bytes_down: number
  connections: number
  /** `ts_ms` самой ранней учтённой записи; записей нет, но профиль работает — запуск прослойки; иначе null. */
  since: number | null
  live: boolean
}

/** Запись журнала прослойки; `result` — `ok` или причина отказа. */
export interface ConnRecord {
  ts_ms: number
  proxy_id: number
  target: string
  bytes_up: number
  bytes_down: number
  ms: number
  result: string
}

/** Отчёт массовой операции: порядок — как в запросе. */
export interface BulkReport {
  ok: string[]
  failed: BulkFailure[]
  /** Массовый запуск: профили, которые ждут поток лицензии. */
  waiting?: string[]
}

export type ProxyChoice = { mode: 'random' } | { mode: 'fixed'; proxy_id: number }

export interface BulkCreateForm {
  count: number
  prefix: string
  group_id: number | null
  tags: string[]
  note: string
  start_urls: string[]
  restore_tabs: boolean
  extra_args: string[]
  proxy: ProxyChoice
}

export type ProxyEdit = { action: 'reset' } | { action: 'set'; proxy_id: number }

/** Массовая правка: незаданные поля не меняются. */
export interface BulkEditForm {
  add_tags?: string[]
  remove_tags?: string[]
  note?: string
  proxy?: ProxyEdit
  start_urls?: string[]
  restore_tabs?: boolean
  extra_args?: string[]
}

export type CookieFormat = 'json' | 'netscape'
export type ExportTarget = { kind: 'file'; path: string } | { kind: 'folder'; path: string }

export type QuitAction = 'ask' | 'stop_all'

export interface ProfileSettings {
  on_quit: QuitAction
}

/** Почему Local API не поднялся (`ApiFailureCode` ядра). */
export type ApiFailureCode = 'port_busy' | 'settings_unreadable' | 'io'

/** `reason` — подробность ядра по-русски, для журнала; интерфейс показывает перевод `code`. */
export type ApiStatus =
  | { state: 'off' }
  | { state: 'listening'; port: number }
  | { state: 'failed'; port: number; code: ApiFailureCode; reason: string }

/** Настройки Local API. `key` — ключ API, когда он требуется, иначе `null`. */
export interface ApiConfig {
  enabled: boolean
  port: number
  require_key: boolean
  key: string | null
  status: ApiStatus
}

export interface ApiConfigForm {
  enabled: boolean
  port: number
  require_key: boolean
}

export interface CookieStats {
  total: number
  /** Записи, которые не удалось расшифровать. */
  unreadable: number
}

/** Почему строка импорта cookies пропущена (`SkipCode` ядра, семь кодов). */
export type SkipCode = 'no_domain' | 'bad_domain' | 'no_name' | 'bad_path' | 'control' | 'bad_expires' | 'unparsed'

/** Пропущенная строка импорта. `reason` — подробность ядра по-русски, для журнала; интерфейс показывает перевод `code`. */
export interface Skipped {
  line: number
  code: SkipCode
  reason: string
}

export interface CookieImportReport {
  added: number
  replaced: number
  skipped: Skipped[]
  sent_to_browser: boolean
}

/** Импорт cookies: текст из поля или путь к файлу (файл читает ядро). */
export type CookieSource = { kind: 'text'; text: string } | { kind: 'file'; path: string }

/** Запись в диалоге прокси. `password` — как в редакторе: оставить, удалить или задать. */
export interface ProxyEditForm {
  kind: ProxyKind
  host: string
  port: number
  username: string
  password: SecretInput
  note: string
  enabled: boolean
}

/** Строка импорта, которую не удалось разобрать: номер с 1 и код (`format`, `kind`, `port`). */
export interface LineError {
  line: number
  code: string
}

export interface ProxyImportReport {
  added: number
  duplicates: number
  errors: LineError[]
  /** id добавленных записей в порядке строк. */
  ids: number[]
}

export interface CsvImportReport {
  created: ProfileRecord[]
  /** `code`: name_required, name_invalid, tags_invalid, start_urls_invalid, extra_args_invalid, proxy_invalid, group_invalid, totp_invalid, format. */
  errors: LineError[]
}

export type VarType = 'string' | 'number' | 'boolean' | 'array' | 'object' | 'text' | 'file' | 'folder'
export interface VarDecl {
  name: string
  type: VarType
  default: unknown
  description: string
}
export type NodePolicy = 'interrupt' | 'skip' | 'branch'
export interface FlowEdge {
  from: string
  port?: string
  to: string
  to_port?: string
}
export interface FlowNode {
  id: string
  op: string
  config?: Record<string, unknown>
  description?: string
  disabled?: boolean
  on_error?: NodePolicy
  pos?: [number, number]
  nodes?: FlowNode[]
  edges?: FlowEdge[]
}
export interface FlowSettings {
  on_error?: 'interrupt' | 'skip'
  after_tabs?: 'clear' | 'keep'
  after_browser?: 'quit' | 'keep'
  headless?: boolean
  timeout_s?: number
  delay_min?: number
  launch_args?: string[]
  clear_cache_on_quit?: boolean
  disable_password_autofill?: boolean
  save_password?: boolean
}
/** Процесс как в ядре (Rust: flow::model::Flow). */
export interface FlowDoc {
  format: string
  name: string
  group?: string
  remark?: string
  variables?: VarDecl[]
  settings?: FlowSettings
  nodes: FlowNode[]
  edges?: FlowEdge[]
  subflows?: Record<string, FlowDoc>
}
export interface FieldInfo {
  name: string
  kind: string
  options: string[]
  required: boolean
  default: unknown
}
export interface OpInfo {
  id: string
  adspower: string
  group: string
  inputs: string[]
  outputs: string[]
  fields: FieldInfo[]
}
export interface OpsCatalog {
  ops: OpInfo[]
  system: string[]
}
export interface Issue {
  node: string | null
  field: string | null
  message: string
}
export interface Report {
  errors: Issue[]
  warnings: Issue[]
}
export interface FlowRow {
  headless?: boolean
  id: number
  name: string
  group: string
  remark: string
  created_at: number
  updated_at: number
  has_errors: boolean
  variables: VarDecl[]
}

export type RunState = 'queued' | 'waiting' | 'running' | 'done' | 'error' | 'cancelled' | 'skipped'
export interface BatchSummary {
  id: number
  started_at: number
  finished_at: number | null
  total: number
  queued: number
  waiting: number
  running: number
  done: number
  error: number
  cancelled: number
  skipped: number
}
export interface TaskRow {
  distribution: 'per_profile' | 'queue'
  queue: { total: number; done: number } | null
  id: number
  name: string
  flow_id: number | null
  flow_name: string
  profiles: number
  threads: number
  order: 'ordered' | 'random'
  missing_row: 'skip' | 'random'
  created_at: number
  running: boolean
  last: BatchSummary | null
}
export interface RunRow {
  item_index: number | null
  id: number
  profile_id: string
  serial: number
  name: string
  position: number
  status: RunState
  error: string | null
  started_at: number | null
  finished_at: number | null
}
/** Пустые строки ядро не шлёт. */
export interface JournalEntry {
  ts_ms: number
  node?: string
  op?: string
  desc?: string
  status: 'start' | 'end' | 'ok' | 'skip' | 'error' | 'branch' | 'disabled'
  ms: number
  msg?: string
  /** Глубина вызова (`control.call`): строка узла подпроцесса; нет — сам процесс. */
  depth?: number
}
export interface TaskForm {
  /** null / отсутствие — режим процесса. */
  headless?: boolean | null
  distribution: 'per_profile' | 'queue'
  name: string | null
  flow_id: number
  profile_ids: string[]
  params: Record<string, unknown>
  data_path: string | null
  threads: number
  order: 'ordered' | 'random'
  missing_row: 'skip' | 'random'
}
export interface TaskSettingsForm {
  name: string
  threads: number
}
export interface TaskDetails {
  headless?: boolean | null
  id: number
  revision: number
  name: string
  flow_id: number | null
  flow: FlowDoc | null
  process: FlowRow | null
  profiles: string[]
  distribution: 'per_profile' | 'queue'
  params: Record<string, unknown>
  data: Record<string, unknown>[] | null
  threads: number
  order: 'ordered' | 'random'
  missing_row: 'skip' | 'random'
  created_at: number
}
export interface TaskUpdateForm extends Omit<TaskForm, 'flow_id' | 'name'> {
  revision: number
  name: string
  flow_id: number | null
  data: Record<string, unknown>[] | null
}
export interface TableDataPreviewForm extends Omit<TablePreviewForm, 'data_path'> {
  data: Record<string, unknown>[]
}
export interface TablePreviewForm {
  distribution: 'per_profile' | 'queue'
  profile_ids: string[]
  data_path: string
  missing_row: 'skip' | 'random'
}
/** `key`: `serial_number`, `acc_id` или `order`; `skipped` — номера профилей без строки. */
export interface TablePreview {
  duplicates: number
  columns: string[]
  rows: number
  key: string
  matched: number
  skipped: number[]
}
/** Режим синхронизатора окон. */
export type SyncMode = 'full_copy' | 'varied'

export interface SyncSettings {
  mode: SyncMode
  clicks: boolean
  typing: boolean
  scroll: boolean
  mouse: boolean
  /** Разброс задержки, мс: `[от, до]`. */
  click_delay: [number, number]
  typing_delay: [number, number]
}

/** Статус окна сессии; `code` — ключ причины (`sync.reason.*`), `detail` подставляется в её текст. */
export type SyncWindowStatus =
  | { state: 'synced' }
  | { state: 'paused'; code: string; detail: string }
  | { state: 'dropped'; code: string }

export interface SyncWindow {
  profileId: string
  main: boolean
  status: SyncWindowStatus
  /** У ведомого нет очеловечивания: повтор без траекторий. */
  plain: boolean
}

/** Состояние сессии синхронизатора (команда `sync_state`, событие `sync`). */
export interface SyncState {
  active: boolean
  mode: SyncMode
  main: string | null
  windows: SyncWindow[]
}

/** Событие `tasks-changed`; `error` — почему пакет не запустился. */
export interface TasksChanged {
  task_id: number
  error: string | null
}

/** Событие `flow-debug`: `phase` — `node` (узел начат), `entry` (строка журнала) или `end` (отладка окончена). */
export interface DebugEvent {
  kind: 'flow_debug'
  run_id: string
  phase: 'node' | 'entry' | 'end'
  node: string | null
  entry: JournalEntry | null
  status: 'done' | 'error' | 'cancelled' | null
  error: string | null
}
