import type { SupervisorEvent } from '../api/types'
import { i18n } from '../i18n'
import { notify } from './notify'
import { CoreError } from '../api/errors'

/** «Шлюз не отвечает» по одному профилю — не чаще раза в минуту. */
export const UNREACHABLE_INTERVAL_MS = 60_000

export interface SupervisorNoticeDeps {
  /** Имя из загруженных списков; `null` — профиля в них нет (скрыт фильтром или списки ещё не пришли). */
  nameOf: (id: string) => string | null
  /** Имя из ядра (`get_profile`); `null` или ошибка — показывается id. */
  fetchName: (id: string) => Promise<string | null>
  now?: () => number
}

/** Уведомления о падении, паузе и замене шлюза. Без Vue: имена и часы передаёт `App.vue`. */
export function createSupervisorNotices(deps: SupervisorNoticeDeps) {
  const now = deps.now ?? Date.now
  const lastUnreachable = new Map<string, number>()
  let engineUpdateOffered = false

  async function nameFor(id: string): Promise<string> {
    const known = deps.nameOf(id)
    if (known !== null) return known
    try {
      return (await deps.fetchName(id)) ?? id
    } catch {
      return id
    }
  }

  async function handle(event: SupervisorEvent): Promise<void> {
    const { t } = i18n.global
    if (event.kind === 'gateway_replaced') {
      notify.warning(t('toasts.gatewayReplaced', { name: event.name }), t('toasts.gatewayReplacedText'))
    } else if (event.kind === 'gateway_unreachable') {
      // Частота считается до запроса имени: повторное событие не ходит в ядро.
      const at = now()
      if (at - (lastUnreachable.get(event.profile_id) ?? -Infinity) < UNREACHABLE_INTERVAL_MS) return
      lastUnreachable.set(event.profile_id, at)
      const name = await nameFor(event.profile_id)
      notify.warning(t('toasts.gatewayUnreachable', { name }), t('toasts.gatewayUnreachableText'))
    } else if (event.status.state === 'waiting' && !engineUpdateOffered) {
      engineUpdateOffered = true
      notify.error(new CoreError('EngineUpdateRequired', ''))
    } else if (event.status.state === 'crashed') {
      const code = event.status.code
      const name = await nameFor(event.profile_id)
      notify.warning(t('toasts.crashed', { name }), code !== null ? t('toasts.crashedCode', { code }) : undefined)
    }
  }

  return { handle }
}
