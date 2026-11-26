import { createPinia, setActivePinia } from 'pinia'
import { beforeEach } from 'vitest'

// Свежий pinia на каждый тест: сторы не делят состояние.
beforeEach(() => setActivePinia(createPinia()))
