<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { Language } from '../../api/types'
import UiSelect from '../../components/UiSelect.vue'
import { LANGUAGES } from '../../i18n'
import { notify } from '../../lib/notify'
import { useAppStore } from '../../stores/app'
import { useSettingsSaveStore } from '../../stores/settingsSave'
const { t } = useI18n()
const app = useAppStore()
const saves = useSettingsSaveStore()
const languages = computed(() => LANGUAGES.map((value) => ({ value, label: t(`languages.${value}`) })))
const language = computed({
  get: () => app.info?.language ?? 'ru',
  set: (value: string) => {
    saves.run('general.language', () => app.setLanguage(value as Language)).catch((error: unknown) => notify.error(error))
  },
})
</script>

<template>
  <section class="card section">
    <h2 class="section-title">{{ t('settings.sections.general') }}</h2>
    <div class="setting-row">
      <div class="setting-intro">
        <label for="ui-language">{{ t('settings.data.language') }}</label>
        <p class="hint">{{ t('settings.general.languageHint') }}</p>
      </div>
      <div class="language"><UiSelect id="ui-language" v-model="language" :disabled="saves.status('general') === 'saving'" :options="languages" /></div>
    </div>
  </section>
</template>

<style scoped>
.language { flex: 0 0 220px; width: 220px; max-width: 100%; }
</style>
