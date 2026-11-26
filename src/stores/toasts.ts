import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface Toast {
  id: number
  kind: 'error' | 'warning' | 'info'
  title: string
  body?: string
  details?: string
  action?: { label: string; to: string }
}

export const useToasts = defineStore('toasts', () => {
  const items = ref<Toast[]>([])
  let next = 1

  function push(toast: Omit<Toast, 'id'>) {
    items.value.push({ ...toast, id: next++ })
  }

  function dismiss(id: number) {
    items.value = items.value.filter((toast) => toast.id !== id)
  }

  return { items, push, dismiss }
})
