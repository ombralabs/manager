import { type Ref, computed, ref, shallowRef } from 'vue'
import { notify } from './notify'

/**
 * Подтверждение действия над элементом: `ask(item)` открывает диалог, `confirm()` выполняет `run(item)`.
 * Диалог закрывается и при ошибке: она приходит уведомлением. `shown` — последний спрошенный элемент,
 * не обнуляется, пока идёт анимация закрытия.
 */
export function useConfirm<T>(run: (item: T) => Promise<void>) {
  const pending = shallowRef<T | null>(null) as Ref<T | null>
  const shown = shallowRef<T | null>(null) as Ref<T | null>
  const busy = ref(false)
  const open = computed({
    get: () => pending.value !== null,
    set: (value: boolean) => {
      if (!value) pending.value = null
    },
  })

  async function confirm() {
    const item = pending.value
    if (item === null) return
    busy.value = true
    try {
      await run(item)
    } catch (e) {
      notify.error(e)
    } finally {
      busy.value = false
      pending.value = null
    }
  }

  return { pending, shown, busy, open, ask: (item: T) => (pending.value = shown.value = item), confirm }
}
