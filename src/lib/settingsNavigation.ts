/** Navigation and search share stable control IDs; translated copy stays in the dictionaries. */
export const settingsSections = ['general', 'profiles', 'engine', 'security', 'integrations'] as const
export type SettingsSection = (typeof settingsSections)[number]
export function settingsSection(value: unknown): SettingsSection | null {
  return settingsSections.find((section) => section === value) ?? null
}

export const settingsEntries = [
  { section: 'general', target: 'ui-language', label: 'settings.data.language', description: 'settings.general.languageHint', synonyms: 'language' },
  { section: 'profiles', target: 'profile-on-quit', label: 'settings.profiles.onQuit', description: 'settings.profiles.quitHint', synonyms: 'quit' },
  { section: 'profiles', target: 'profile-proxy-rules', label: 'settings.profiles.pauseOnDrop', description: 'settings.profiles.pauseOnDropText', synonyms: 'connection' },
  { section: 'profiles', target: 'profile-proxy-exclusive', label: 'settings.profiles.exclusive', description: 'settings.profiles.exclusiveText', synonyms: 'connection' },
  { section: 'engine', target: 'engine-heading', label: 'settings.engine.title', description: 'settings.engine.missingText', synonyms: 'browser' },
  { section: 'engine', target: 'engine-automatic', label: 'settings.engine.automatic', description: 'settings.engine.keepVersions', synonyms: 'update' },
  { section: 'engine', target: 'engine-versions', label: 'settings.engine.available', description: 'settings.engine.nextLaunch', synonyms: 'versions' },
  { section: 'security', target: 'local-security', label: 'security.title', description: 'security.setupHint', synonyms: 'security' },
  { section: 'security', target: 'data-folder', label: 'settings.data.folder', description: 'settings.data.locationHint', synonyms: 'data' },
  { section: 'integrations', target: 'api-enabled', label: 'settings.api.title', description: 'settings.api.text', synonyms: 'api' },
  { section: 'integrations', target: 'api-port', label: 'settings.api.port', description: 'settings.api.address', synonyms: 'port' },
  { section: 'integrations', target: 'api-require-key', label: 'settings.api.key', description: 'settings.api.keyText', synonyms: 'key' },
  { section: 'integrations', target: 'services-proxy', label: 'settings.automation.proxy', description: 'settings.automation.proxyHint', synonyms: 'proxy' },
  { section: 'integrations', target: 'google-key', label: 'settings.automation.google', description: 'settings.search.googleHint', synonyms: 'google' },
] as const satisfies readonly { section: SettingsSection; target: string; label: string; description: string; synonyms: string }[]
export type SettingsEntry = (typeof settingsEntries)[number]

export function searchSettings(query: string, translate: (key: string) => string): SettingsEntry[] {
  const normalize = (value: string) => value.toLocaleLowerCase().replaceAll('ё', 'е')
  const words = normalize(query).trim().split(/\s+/).filter(Boolean)
  if (!words.length) return []
  return settingsEntries.filter((entry) => {
    const text = normalize([
      translate(entry.label), translate(entry.description), translate(`settings.sections.${entry.section}`),
      translate(`settings.search.synonyms.${entry.synonyms}`),
    ].join(' '))
    return words.every((word) => text.includes(word))
  })
}

/** Old bookmarks remain useful after window sync left global settings. */
export function legacySettingsTarget(hash: string) {
  if (hash === '#sync') return { path: '/sync/settings' }
  if (hash === '#api') return { path: '/settings', hash: '#integrations', query: { focus: 'api-enabled' } }
  if (hash === '#automation') return { path: '/settings', hash: '#integrations', query: { focus: 'services-proxy' } }
  return null
}
