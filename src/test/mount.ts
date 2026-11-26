import { mount } from '@vue/test-utils'
import { getActivePinia } from 'pinia'
import type { Component } from 'vue'
import { i18n } from '../i18n'

type MountOptions = NonNullable<Parameters<typeof mount>[1]>

/**
 * Монтирует компонент с i18n, pinia текущего теста (свежий — см. `setup.ts`, так стор можно наполнить до монтирования)
 * и заглушкой teleport; `options.global` дополняет и перекрывает умолчания.
 */
export function mountWithApp(component: Component, options: MountOptions = {}) {
  const { plugins = [], stubs = {}, ...global } = options.global ?? {}
  return mount(component, {
    ...options,
    global: {
      renderStubDefaultSlot: true,
      ...global,
      plugins: [i18n, getActivePinia()!, ...plugins],
      stubs: { teleport: true, ...stubs },
    },
  })
}
