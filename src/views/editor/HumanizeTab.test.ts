import { flushPromises } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'
import type { PersonaView, ProfileDetails } from '../../api/types'
import { type EditorModel, emptyModel } from '../../lib/profileForm'
import { fingerprint } from '../../test/fixtures'
import { mountWithApp } from '../../test/mount'

const api = vi.hoisted(() => ({ resetPersona: vi.fn() }))
vi.mock('../../api/core', () => ({ core: api, events: {} }))

import HumanizeTab from './HumanizeTab.vue'

const persona: PersonaView = {
  generator_version: 1,
  keys_per_sec: 5.1,
  typo_rate: 0.0125,
  dwell_ms: 98.4,
  mouse_speed: 1.25,
  scroll_device: 'trackpad',
}

/** Модель реактивна, как у редактора: вкладка правит её поля на месте. */
function mountTab(props: { profileId?: string | null; persona?: PersonaView | null; errors?: { field: string; code: string }[] } = {}) {
  const model = reactive<EditorModel>(emptyModel(structuredClone(fingerprint)))
  const wrapper = mountWithApp(HumanizeTab, {
    props: { modelValue: model, errors: props.errors ?? [], profileId: props.profileId ?? null, persona: props.persona ?? null },
  })
  return { wrapper, model }
}

describe('вкладка «Очеловечивание»', () => {
  it('новый профиль: включено, темп обычный, блокировка, клавиши нет; почерк — после сохранения', () => {
    const { wrapper, model } = mountTab()
    expect(model.humanize).toEqual({ enabled: true, preset: 'default', block_input: true, paste_key: '' })
    expect(wrapper.text()).toContain('Стиль ввода будет создан после сохранения профиля')
    expect(wrapper.text()).toContain('Изменения действуют со следующего запуска профиля')
    expect(wrapper.find('#paste-key').attributes('placeholder')).toBe('Не задана')
    expect(wrapper.text()).not.toContain('Создать новый стиль ввода')
  })

  it('горячая клавиша — нажатием, по физической клавише; Backspace снимает; одиночная клавиша и ⌘ не меняют', async () => {
    const { wrapper, model } = mountTab()
    const field = wrapper.find('#paste-key')
    await field.trigger('keydown', { key: 'м', code: 'KeyV', ctrlKey: true, altKey: true })
    expect(model.humanize.paste_key).toBe('Ctrl+Alt+V')
    await field.trigger('keydown', { key: 'x', code: 'KeyX' })
    expect(model.humanize.paste_key).toBe('Ctrl+Alt+V')
    await field.trigger('keydown', { key: 'v', code: 'KeyV', metaKey: true, shiftKey: true })
    expect(model.humanize.paste_key).toBe('Ctrl+Alt+V')
    await field.trigger('keydown', { key: 'Backspace', code: 'Backspace' })
    expect(model.humanize.paste_key).toBe('')
  })

  it('выключенное прячет темп, блокировку и клавишу', async () => {
    const { wrapper, model } = mountTab()
    model.humanize.enabled = false
    await flushPromises()
    expect(wrapper.find('#paste-key').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('Темп')
  })

  it('ошибка горячей клавиши — у её поля', () => {
    const { wrapper } = mountTab({ errors: [{ field: 'humanize.paste_key', code: 'invalid' }] })
    expect(wrapper.text()).toContain('Нажмите Ctrl или Alt (Control или Option в macOS) вместе с латинской буквой или цифрой. Можно добавить Shift. Клавиши Command и Windows не поддерживаются.')
  })

  it('почерк сохранённого профиля строками; «Создать новый стиль ввода» после подтверждения отдаёт новый', async () => {
    const { wrapper } = mountTab({ profileId: 'p1', persona })
    const text = wrapper.text()
    for (const part of ['306 зн/мин', '1,3 % символов', '98 мс', '80 % от средней скорости', 'Тачпад']) {
      expect(text).toContain(part)
    }
    const fresh = { ...persona, keys_per_sec: 7 }
    api.resetPersona.mockResolvedValue({ persona: fresh } as ProfileDetails)
    const reset = wrapper.findAll('button').find((b) => b.text() === 'Создать новый стиль ввода')!
    await reset.trigger('click')
    await flushPromises()
    const confirm = wrapper.findAll('button').filter((b) => b.text() === 'Создать новый стиль ввода').at(-1)!
    await confirm.trigger('click')
    await flushPromises()
    expect(api.resetPersona).toHaveBeenCalledWith('p1')
    expect(wrapper.emitted('reset')).toEqual([[{ persona: fresh }]])
  })
})
