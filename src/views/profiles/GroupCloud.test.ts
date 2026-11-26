import { describe, expect, it } from 'vitest'
import type { GroupRecord } from '../../api/types'
import type { GroupFilter } from '../../stores/profiles'
import GroupCloud from './GroupCloud.vue'
import { mountWithApp } from '../../test/mount'

const shops: GroupRecord = {
  id: 1,
  name: 'Магазины',
  color: '#0a6b66',
  note: '',
  quick_access: true,
  created_at: 1,
  profile_count: 2,
}
const ads: GroupRecord = { ...shops, id: 2, name: 'Реклама', color: '#b02a6b', profile_count: 3 }

function cloud(active: GroupFilter) {
  return mountWithApp(GroupCloud, { props: { groups: [shops, ads], total: 10, active } })
}

function chip(wrapper: ReturnType<typeof cloud>, text: string) {
  const found = wrapper.findAll('button').find((button) => button.text().includes(text))
  if (!found) throw new Error(`нет кнопки «${text}»`)
  return found
}

describe('облако групп', () => {
  it('нажатие на группу ставит её фильтр', async () => {
    const wrapper = cloud('all')
    await chip(wrapper, 'Магазины').trigger('click')
    expect(wrapper.emitted('select')).toEqual([[1]])
  })

  it('повторное нажатие на активную группу снимает фильтр', async () => {
    const wrapper = cloud(1)
    expect(chip(wrapper, 'Магазины').attributes('aria-pressed')).toBe('true')
    await chip(wrapper, 'Магазины').trigger('click')
    expect(wrapper.emitted('select')).toEqual([['all']])
  })

  it('«Все» с числом профилей снимает фильтр', async () => {
    const wrapper = cloud(2)
    expect(chip(wrapper, 'Все').text()).toContain('10')
    expect(chip(wrapper, 'Все').attributes('aria-pressed')).toBe('false')
    await chip(wrapper, 'Все').trigger('click')
    expect(wrapper.emitted('select')).toEqual([['all']])
  })

  it('«Настроить» ведёт к экрану «Группы»', async () => {
    const wrapper = cloud('all')
    await chip(wrapper, 'Настроить').trigger('click')
    expect(wrapper.emitted('configure')).toHaveLength(1)
  })
})
