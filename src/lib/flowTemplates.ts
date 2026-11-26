import type { Language } from '../api/types'
import googleRu from '../../tools/flows/examples/google-search.json?raw'
import googleEn from '../../tools/flows/examples/google-search.en.json?raw'

export const templateCategories = ['search'] as const
export type TemplateCategory = typeof templateCategories[number]
export interface FlowTemplate {
  id: string
  category: TemplateCategory
  sources: Record<Language, string>
  summary: Record<Language, string>
  instructions: Record<Language, readonly string[]>
}

export const flowTemplates: readonly FlowTemplate[] = [{
  id: 'google-search', category: 'search', sources: { ru: googleRu, en: googleEn },
  summary: {
    ru: 'Поиск в Google до 20 страниц с ожиданием CAPTCHA. Глубина настраивается в параметрах.',
    en: 'Search up to 20 Google result pages with CAPTCHA waiting. Set the depth in the parameters.',
  },
  instructions: {
    ru: [
      'Укажите keyword — поисковую фразу, и url — домен или адрес целевой страницы. Оба поля обязательны. Протокол, www, параметры URL и фрагмент не учитываются; домен и путь должны совпадать полностью или по границе сегмента.',
      'Создайте копию, затем задачу для выбранного профиля. Сценарий откроет новую вкладку Google, введёт keyword и нажмёт Enter.',
      'Задайте page_limit — глубину поиска от 1 до 20 страниц, по умолчанию 20. Поиск проверит все ссылки результатов в пределах этой глубины. Если результатов нет, новая вкладка закроется. Если совпадений нет или достигнута последняя страница, поиск завершится.',
      'При совпадении откроется целевой сайт: прокрутка вниз, затем вверх. После этого сценарий нажмёт случайную видимую HTTPS-ссылку того же сайта, повторит прокрутку и вернётся назад. Ссылки на скачивание и явные служебные действия исключены. Если подходящих ссылок нет, процесс закончится на целевой странице.',
      'Если Google покажет CAPTCHA или согласие, процесс будет ждать до трёх минут и продолжит после проверки. Дайте встроенному обработчику капч профиля закончить работу или пройдите проверку вручную. Скрытые фоновые виджеты не останавливают поиск. Остальные вкладки не закрываются; браузер остаётся открытым.',
    ],
    en: [
      'Set keyword to the search phrase and url to the target domain or page address. Both are required. Matching ignores protocol, www, query and fragment; domain and path must match on a domain or path-segment boundary.',
      'Create a copy, then a task for the selected profile. The workflow opens a new Google tab, types keyword and presses Enter.',
      'Set page_limit to a search depth from 1 to 20 pages, default 20. The search checks all result links within this depth. Empty results close the new tab. No match or the last page ends the search.',
      'A match opens the target site and scrolls down, then up. The workflow then clicks a random visible same-site HTTPS link, repeats scrolling and goes back. Downloads and recognizable service actions are excluded. If no eligible links exist, it finishes on the target page.',
      'If Google shows CAPTCHA or consent, the workflow waits up to three minutes and resumes after the check. Let the profile’s built-in CAPTCHA handler finish or complete the check manually. Hidden background widgets do not stop the search. Other tabs are preserved and the browser stays open.',
    ],
  },
}]
