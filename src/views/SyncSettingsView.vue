<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import AppIcon from '../components/AppIcon.vue'
import SettingsSaveStatus from '../components/SettingsSaveStatus.vue'
import SyncCard from './settings/SyncCard.vue'
const { t } = useI18n()
</script>

<template>
  <main class="page sync-settings-page">
    <RouterLink class="back" to="/sync"><AppIcon name="prev" :size="18" />{{ t('sync.title') }}</RouterLink>
    <h1 class="page-title">{{ t('sync.settingsTitle') }}</h1>
    <div class="column"><SettingsSaveStatus scope="sync" /><SyncCard /></div>
  </main>
</template>

<style scoped>
.back { display: flex; align-items: center; gap: 6px; min-height: 44px; align-self: flex-start; text-decoration: none; }
.back:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.column { display: flex; flex-direction: column; gap: 16px; max-width: 800px; min-width: 0; }
.column > .section { padding: 24px; }
@media (max-width: 700px) { .sync-settings-page { padding: 20px 16px; } .column > .section { padding: 16px; } }
</style>
