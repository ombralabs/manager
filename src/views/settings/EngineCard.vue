<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { pickEngineArchive } from '../../api/dialogs'
import UiButton from '../../components/UiButton.vue'
import UiSwitch from '../../components/UiSwitch.vue'
import { engineMajor } from '../../lib/format'
import { errorTitle } from '../../lib/errorText'
import { notify } from '../../lib/notify'
import { useAppStore } from '../../stores/app'
import { useEnginesStore } from '../../stores/engines'

const { t, locale } = useI18n()
const app = useAppStore()
const engines = useEnginesStore()
const engine = computed(() => app.info?.engine ?? null)
const selected = computed(() => engines.catalog?.versions.find((version) => version.active))
const major = computed(() => engineMajor(engine.value?.version))
const automatic = computed({
  get: () => engines.catalog?.automatic ?? true,
  set: (value: boolean) => { void engines.automatic(value) },
})
const percent = computed(() => {
  const progress = engines.progress
  return progress?.total ? Math.min(100, Math.floor(progress.downloaded * 100 / progress.total)) : undefined
})
const progressText = computed(() => engines.progress
  ? t(`settings.engine.phases.${engines.progress.phase}`, { version: engines.progress.version ?? '' }) : '')
const canCancel = computed(() => engines.operation === 'update' && ['checking', 'downloading', 'verifying'].includes(engines.progress?.phase ?? ''))
function megabytes(bytes: number) {
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: 1 }).format(bytes / 1024 / 1024)
}

async function chooseArchive() {
  try {
    const path = await pickEngineArchive()
    if (path !== null && await engines.importArchive(path)) notify.info(t('settings.engine.installed'))
  } catch (error) { notify.error(error) }
}

onMounted(() => { void engines.load() })
</script>

<template>
  <section class="card section" aria-labelledby="engine-heading">
    <h2 id="engine-heading" tabindex="-1" class="section-title">{{ t('settings.engine.title') }}</h2>
    <div class="engine-line">
      <span class="engine-name">Ombra</span>
      <span v-if="major" class="mono">Chromium {{ major }}</span>
      <span :class="['pill', engine ? 'pinned' : 'missing']">
        {{ engine ? t('settings.engine.pinned') : t('settings.engine.missing') }}
      </span>
    </div>
    <p class="hint">
      {{ engine ? t('settings.engine.installedText', { version: selected?.version ?? engine.version }) : t('settings.engine.missingText') }}
    </p>
    <div class="actions">
      <UiButton variant="primary" :icon="engine ? 'refresh' : 'download'" :loading="engines.operation === 'update'"
        :busy="engines.busy && engines.operation !== 'update'" @click="engines.update()">
        {{ engine ? t('settings.engine.checkUpdate') : t('settings.engine.download') }}
      </UiButton>
      <UiButton icon="download" :loading="engines.operation === 'import'" :busy="engines.busy && engines.operation !== 'import'" @click="chooseArchive">
        {{ engines.operation === 'import' ? t('settings.engine.installing') : t('settings.engine.install') }}
      </UiButton>
    </div>
    <div v-if="engines.progress" class="download-progress">
      <div class="progress-line">
        <span role="status" aria-live="polite">{{ progressText }}</span>
        <UiButton v-if="canCancel" size="sm" :busy="false" @click="engines.cancel()">{{ t('common.cancel') }}</UiButton>
      </div>
      <progress v-if="engines.progress.phase === 'downloading'" :value="percent" max="100" :aria-label="progressText" />
      <span v-if="engines.progress.phase === 'downloading' && engines.progress.total" class="hint mono">
        {{ megabytes(engines.progress.downloaded) }} / {{ megabytes(engines.progress.total) }} {{ t('settings.engine.mb') }} · {{ percent }}%
      </span>
    </div>
    <p v-if="engines.errorCode" class="error" role="alert">{{ errorTitle(engines.errorCode) }}</p>
    <p v-else-if="engines.result && !engines.busy" class="hint" role="status">{{ t(`settings.engine.results.${engines.result}`) }}</p>
    <div v-if="engines.catalog" class="automatic-row">
      <div>
        <label for="engine-automatic">{{ t('settings.engine.automatic') }}</label>
        <p class="hint">{{ t(automatic ? 'settings.engine.keepVersions' : 'settings.engine.manualPin') }}</p>
      </div>
      <UiSwitch id="engine-automatic" v-model="automatic" :label="t('settings.engine.automatic')" :aria-disabled="engines.busy || undefined" />
    </div>
    <div id="engine-versions" tabindex="-1" class="versions">
      <template v-if="engines.catalog?.versions.length">
      <h3 class="versions-title">{{ t('settings.engine.available') }}</h3>
      <ul :aria-label="t('settings.engine.available')">
        <li v-for="version in engines.catalog.versions" :key="version.id">
          <div class="version-name">
            <span class="mono">{{ version.version }}</span>
            <span v-if="version.id === 'legacy'" class="hint">{{ t('settings.engine.local') }}</span>
          </div>
          <span v-if="version.active" class="pill pinned">{{ t('settings.engine.active') }}</span>
          <UiButton v-else size="sm" :busy="engines.busy" :aria-label="t('settings.engine.useVersion', { version: version.version })" @click="engines.select(version.id)">
            {{ t('settings.engine.use') }}
          </UiButton>
        </li>
      </ul>
      <p class="hint">{{ t('settings.engine.nextLaunch') }}</p>
      </template>
      <p v-else-if="engines.loading" class="hint" role="status">{{ t('settings.engine.loading') }}</p>
      <p v-else class="hint">{{ t('settings.engine.noVersions') }}</p>
    </div>
  </section>
</template>

<style scoped>
.engine-line, .actions, .progress-line, .automatic-row, .versions li {
  display: flex;
  align-items: center;
  gap: 12px;
}
.engine-line, .actions { flex-wrap: wrap; }
.engine-name { font-weight: 600; }
.pill { padding: 0 8px; border-radius: 999px; font-size: 12px; line-height: 24px; font-weight: 600; white-space: nowrap; }
.pill.pinned { background: var(--muted-bg); color: var(--text-2); }
.pill.missing { background: var(--danger-bg); color: var(--danger-text); }
p { margin: 0; }
.progress-line, .automatic-row, .versions li { justify-content: space-between; }
.download-progress { display: grid; gap: 8px; }
progress { width: 100%; height: 8px; accent-color: var(--accent); }
.error { color: var(--danger-text); }
.automatic-row { padding-top: 16px; border-top: 1px solid var(--border); }
.automatic-row label { display: block; margin-bottom: 4px; font-weight: 500; cursor: pointer; }
.versions-title { margin: 0 0 6px; font-size: 13px; font-weight: 600; color: var(--text-2); }
.versions ul { list-style: none; padding: 0; margin: 0 0 8px; }
.versions li { min-height: 48px; padding: 6px 0; border-bottom: 1px solid var(--border); }
.version-name { display: flex; flex-wrap: wrap; gap: 6px 12px; min-width: 0; overflow-wrap: anywhere; }
</style>
