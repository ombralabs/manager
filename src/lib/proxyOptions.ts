import type { ProxyKind, ProxyRow } from '../api/types'
import type { SelectOption } from '../components/types'
import { kindLabel } from './format'
import { proxyAddress } from './proxies'

/**
 * Прокси для выпадающего списка: «SOCKS5 · gw1.example.net:1080 · DE · заметка». Логин не показывается,
 * выключенный помечен `offLabel`. С `busyLabel` прокси, который держит запущенный профиль, помечается
 * «занят: <имя>» — кроме профиля `self`: редактор не помечает собственный прокси.
 */
export function proxyOptions(
  proxies: ProxyRow[],
  placeholder: string,
  offLabel: string,
  busyLabel?: (name: string) => string,
  self: string | null = null,
): SelectOption[] {
  return [
    { value: '', label: placeholder },
    ...proxies.map((proxy) => {
      const parts = [kindLabel(proxy.kind), proxyAddress(proxy)]
      if (proxy.check?.country) parts.push(proxy.check.country)
      if (proxy.note) parts.push(proxy.note)
      if (!proxy.enabled) parts.push(offLabel)
      if (busyLabel && proxy.used_by !== null && proxy.used_by !== self) {
        parts.push(busyLabel(proxy.used_by_name ?? proxy.used_by))
      }
      return { value: String(proxy.id), label: parts.join(' · ') }
    }),
  ]
}

const KINDS: ProxyKind[] = ['http', 'https', 'socks5']
export const kindOptions: SelectOption<ProxyKind>[] = KINDS.map((kind) => ({ value: kind, label: kindLabel(kind) }))
