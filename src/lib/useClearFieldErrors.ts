import { type Ref, type WatchSource, watch } from 'vue'
import type { FieldError } from '../api/types'
import { clearFieldError } from './profileForm'

/**
 * Правка поля снимает его ошибку, ошибки других полей остаются. `snapshot(field)` — значение поля строкой
 * (`''` у неизвестного поля: его ошибка не снимается); сравнивается с моментом, когда ошибки были выставлены.
 * `source` — что наблюдать глубоко (модель или черновик формы).
 */
export function useClearFieldErrors(errors: Ref<FieldError[]>, source: WatchSource<object | null>, snapshot: (field: string) => string) {
  let shown = new Map<string, string>()
  watch(errors, (list) => (shown = new Map(list.map((error) => [error.field, snapshot(error.field)]))), { flush: 'sync' })
  watch(
    source,
    () => {
      let next = errors.value
      for (const error of errors.value) {
        if (snapshot(error.field) !== shown.get(error.field)) next = clearFieldError(next, error.field)
      }
      if (next !== errors.value) errors.value = next
    },
    { deep: true },
  )
}
