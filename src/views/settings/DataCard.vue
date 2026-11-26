<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import UiButton from '../../components/UiButton.vue'
import { notify } from '../../lib/notify'
import { useAppStore } from '../../stores/app'
import { useSecurityStore } from '../../stores/security'
const { t } = useI18n()
const app = useAppStore()
const security = useSecurityStore()

async function reveal() {
  try {
    await core.revealDataDir()
  } catch (e) {
    notify.error(e)
  }
}
</script>

<template>
  <section class="card section">
    <h2 class="section-title">{{ t('settings.data.title') }}</h2>
    <div id="data-folder" class="field" tabindex="-1">
      <div class="label">{{ t('settings.data.folder') }}</div>
      <div class="folder">
        <div class="path mono">{{ app.info?.data_dir }}</div>
        <UiButton @click="reveal">{{ t('settings.data.reveal') }}</UiButton>
      </div>
      <div class="hint">{{ t(security.status?.migrating ? 'security.migrationPendingHint' : security.status?.configured ? 'settings.data.protected' : 'settings.data.unprotected') }}</div>
    </div>
  </section>
</template>

<style scoped>
.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.label {
  font-size: 13px;
  line-height: 18px;
  color: var(--text-2);
}

.folder {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
}

.path {
  flex: 1 1 260px;
  min-width: 0;
  height: var(--control-h);
  line-height: calc(var(--control-h) - 2px);
  padding: 0 12px;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: var(--radius-control);
  background: var(--bg);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}


</style>
