import type { ExtCategory, ExtensionRecord, Fingerprint, GroupRecord, ProfileLabel, ProfileRow, ProxyRow } from '../api/types'

export const profileLabel = (patch: Partial<ProfileLabel> = {}): ProfileLabel => ({ id: 2, name: 'READY', color: '#237a36', preset: 'ready', ...patch })

export const profileRow = (patch: Partial<ProfileRow> = {}): ProfileRow => ({
  id: 'p1',
  serial: 1,
  name: 'Профиль 1',
  note: '',
  group: null,
  tags: [],
  label: null,
  gateway: null,
  gateway_replaced_at: null,
  restore_tabs: true,
  os_version: '15.2.0',
  brand: 'Chrome',
  seed: 1,
  status: { state: 'stopped' },
  created_at: 1,
  last_opened_at: null,
  deleted_at: null,
  ext_category_id: 0,
  ...patch,
})

export const proxyRow = (patch: Partial<ProxyRow> = {}): ProxyRow => ({
  id: 1,
  kind: 'http',
  host: 'gw1.example.net',
  port: 8080,
  username: null,
  has_password: false,
  note: '',
  enabled: true,
  check: null,
  profiles: 0,
  used_by: null,
  used_by_name: null,
  pinned_to: null,
  ...patch,
})

export const group = (patch: Partial<GroupRecord> = {}): GroupRecord => ({
  id: 1,
  name: 'Группа 1',
  color: '#2447c9',
  note: '',
  quick_access: false,
  created_at: 1,
  profile_count: 0,
  ...patch,
})

export const fingerprint: Fingerprint = {
  seed: 48213,
  os: 'macos',
  os_version: '15.2.0',
  brand: 'Chrome',
  gpu: { vendor: 'Google Inc. (Apple)', renderer: 'ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)' },
  hardware_concurrency: 8,
  device_memory: 16,
  screen: { width: 1512, height: 982 },
  timezone: { mode: 'auto' },
  language: { mode: 'auto' },
  webrtc: { mode: 'auto' },
  noise: true,
  storage_quota_mb: null,
  device_pixel_ratio: 2,
  max_touch_points: 0,
  location: { mode: 'off' },
  manual: [],
}

export const extension = (patch: Partial<ExtensionRecord> = {}): ExtensionRecord => ({
  id: 'cjpalhdlnbpafiamejdnhcphjbkeiagm',
  name: 'uBlock Origin',
  version: '1.62.0',
  source: 'store',
  manifest_version: 2,
  gateway_risk: false,
  added_at: 1_700_000_000,
  updated_at: 1_700_000_000,
  description: '',
  file_name: null,
  ...patch,
})

/** Расширение с правом `proxy`: пометка `gateway_risk`. */
export const riskyExtension = extension({
  id: 'pfnededegaaopdmhkdmcofjmoldfiped',
  name: 'ZeroOmega',
  version: '3.3.23',
  manifest_version: 3,
  gateway_risk: true,
})

export const extCategory = (patch: Partial<ExtCategory> = {}): ExtCategory => ({
  id: 0,
  name: 'Общий',
  remark: '',
  extension_ids: [],
  profile_count: 0,
  ...patch,
})
