import { ref } from 'vue'
import type { BulkReport } from '../api/types'
import { i18n } from '../i18n'
import { useGroupsStore } from '../stores/groups'
import { useProfilesStore } from '../stores/profiles'
import { useToasts } from '../stores/toasts'
import { type ReportLine, reportLines } from './bulk'

export type BulkKind = 'start' | 'stop' | 'delete' | 'move' | 'edit' | 'clone' | 'export' | 'restore' | 'purge' | 'arrange' | 'extSet'

/** Отчёт массовой операции с ошибками: заголовок «Запущено 18, не запущено 2» и строки «профиль — причина». */
interface ReportView {
  title: string
  /** Профили из отчёта есть в списке «Профилей»; у корзины их там нет. */
  inList: boolean
  lines: ReportLine[]
}

/** Открытый отчёт; `null` — диалог закрыт. */
export const currentReport = ref<ReportView | null>(null)

/** Имя из списка экрана, затем из полного списка хранилища групп (профиль может быть скрыт фильтром); `null` — нет в обоих. */
export function profileName(id: string): string | null {
  return (
    useProfilesStore().rows.find((row) => row.id === id)?.name ?? useGroupsStore().rows.find((row) => row.id === id)?.name ?? null
  )
}

/**
 * Итог массовой операции: всё прошло — уведомление («Запущено 18»), были ошибки — диалог отчёта.
 * `name` — имя по id; по умолчанию — из хранилищ «Профилей» и «Групп» (корзина передаёт свой список).
 */
export function showBulkReport(kind: BulkKind, report: BulkReport, name: (id: string) => string | null = profileName): void {
  const waiting = report.waiting?.length ?? 0
  if (report.failed.length === 0 && (report.ok.length > 0 || waiting === 0)) {
    useToasts().push({ kind: 'info', title: i18n.global.t(`bulk.report.done.${kind}`, report.ok.length) })
  }
  // Массовый запуск: ждущие поток — не ошибка, их запуск идёт дальше сам.
  if (waiting > 0) useToasts().push({ kind: 'info', title: i18n.global.t('bulk.report.waiting', waiting) })
  if (report.failed.length === 0) return
  currentReport.value = {
    title: i18n.global.t(`bulk.report.partial.${kind}`, { ok: report.ok.length, failed: report.failed.length }),
    inList: kind !== 'restore' && kind !== 'purge',
    lines: reportLines(report, (id) => name(id) ?? id),
  }
}
