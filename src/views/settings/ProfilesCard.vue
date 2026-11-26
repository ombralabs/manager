<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import UiSegmented from '../../components/UiSegmented.vue'
import { notify } from '../../lib/notify'
import { useSettingsStore } from '../../stores/settings'

const { t } = useI18n()
const settings = useSettingsStore()

const quitOptions = computed(() => [
  { value: 'ask', label: t('settings.profiles.quitAsk') },
  { value: 'stop_all', label: t('settings.profiles.quitStopAll') },
])
const quitModel = computed({
  get: () => settings.profile?.on_quit ?? 'ask',
  set: (value: string) => {
    settings.setOnQuit(value === 'stop_all' ? 'stop_all' : 'ask').catch((e: unknown) => notify.error(e))
  },
})

/** Ядро соблюдает оба правила всегда. */
const rules = computed(() => [
  { label: t('settings.profiles.pauseOnDrop'), text: t('settings.profiles.pauseOnDropText') },
  { label: t('settings.profiles.exclusive'), text: t('settings.profiles.exclusiveText') },
])
</script>

<template>
  <section v-if="settings.profile" class="card section">
    <h2 class="section-title title">{{ t('settings.profiles.title') }}</h2>
    <div class="row">
      <div class="name">{{ t('settings.profiles.onQuit') }}</div>
      <div id="profile-on-quit" class="quit" tabindex="-1">
        <UiSegmented v-model="quitModel" :options="quitOptions" :aria-label="t('settings.profiles.onQuit')" />
      </div>
    </div>
    <div v-for="(rule, index) in rules" :id="index === 0 ? 'profile-proxy-rules' : 'profile-proxy-exclusive'" :key="rule.label" class="row rule" tabindex="-1">
      <div class="intro">
        <div class="name">{{ rule.label }}</div>
        <div class="hint">{{ rule.text }}</div>
      </div>
      <span class="always">{{ t('settings.profiles.alwaysOn') }}</span>
    </div>
  </section>
</template>

<style scoped>
/* Строки по 64 px с разделителями, подпись слева, элемент справа (холст «Настройки»). */
.section {
  gap: 0;
}

.title {
  margin-bottom: 8px;
}

.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  min-height: 64px;
  border-bottom: 1px solid var(--row-border);
}

.row:last-child {
  border-bottom: 0;
}

.name {
  font-weight: 500;
}

.quit {
  width: 280px;
  max-width: 100%;
  flex-shrink: 0;
}

.always {
  flex-shrink: 0;
  color: var(--text-2);
  font-size: 13px;
}

.intro {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.intro p { margin: 2px 0 0; }
@media (max-width: 700px) {
  .row { flex-wrap: wrap; padding-block: 16px; gap: 12px; }
}
</style>
