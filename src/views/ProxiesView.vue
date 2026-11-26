<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import type { ProxyRow } from '../api/types'
import AppIcon from '../components/AppIcon.vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import ProxyDialog from '../components/ProxyDialog.vue'
import UiButton from '../components/UiButton.vue'
import { notify } from '../lib/notify'
import { useConfirm } from '../lib/useConfirm'
import { proxyAddress } from '../lib/proxies'
import { useProxiesStore } from '../stores/proxies'
import ImportPanel from './proxies/ImportPanel.vue'
import ProxiesTable from './proxies/ProxiesTable.vue'

const { t } = useI18n()
const proxies = useProxiesStore()

/** Запись диалога не обнуляется при закрытии: заголовок не меняется во время анимации. */
const dialogOpen = ref(false)
const dialogProxy = ref<ProxyRow | null>(null)

function openDialog(row: ProxyRow | null) {
  dialogProxy.value = row
  dialogOpen.value = true
}

function toggle(row: ProxyRow, enabled: boolean) {
  proxies.setEnabled(row.id, enabled).catch((e) => notify.error(e))
}

/** Итог виден в строке; ошибка — уведомлением (при `GeoUnavailable` строка не меняется). */
async function checkOne(row: ProxyRow) {
  try {
    await proxies.check(row.id)
  } catch (e) {
    notify.error(e)
  }
}

/** «Проверить все» неактивна, пока идёт любая проверка. */
const checkBusy = computed(() => proxies.checkingAll || proxies.checking.size > 0)

async function checkAll() {
  const { ok, total } = await proxies.checkAll()
  notify.info(t('proxies.checkAllDone', { ok, total }))
}

const del = useConfirm(async (row: ProxyRow) => {
  await proxies.remove(row.id)
  notify.info(t('proxies.deleted'))
})
const deleteProfiles = computed(() => del.shown.value?.profiles ?? 0)
const deleteText = computed(() =>
  deleteProfiles.value > 0 ? t('proxies.deleteText', { n: deleteProfiles.value }) : t('proxies.deleteTextFree'),
)

onMounted(() => {
  proxies.refresh().catch((e) => notify.error(e))
})

onBeforeUnmount(() => proxies.cancelTimers())
</script>

<template>
  <main class="page list-page proxies-page">
    <header class="head">
      <div class="title-group">
        <h1 class="page-title">{{ t('proxies.title') }}</h1>
        <div class="hint">
          {{ t('proxies.records', proxies.total) }} · {{ t('proxies.pinnedCount', proxies.pinnedCount) }}
        </div>
      </div>
      <div class="head-actions">
        <UiButton icon="refresh" :loading="proxies.checkingAll" :disabled="proxies.total === 0 || checkBusy" @click="checkAll">
          {{ t('proxies.checkAll') }}
        </UiButton>
        <UiButton variant="primary" icon="plus" @click="openDialog(null)">{{ t('proxies.add') }}</UiButton>
      </div>
    </header>
    <p class="note"><AppIcon name="info" />{{ t('proxies.info') }}</p>

    <div class="layout">
      <section class="table card" :aria-label="t('proxies.title')">
        <ProxiesTable
          :rows="proxies.rows"
          :checking="proxies.checking"
          @toggle="toggle"
          @check="checkOne"
          @edit="openDialog"
          @remove="del.ask($event)"
        >
          <template #empty>
            <template v-if="proxies.loaded">
              <div class="section-title">{{ t('proxies.empty') }}</div>
              <div class="hint">{{ t('proxies.emptyHint') }}</div>
            </template>
          </template>
        </ProxiesTable>
      </section>
      <ImportPanel />
    </div>

    <ProxyDialog v-model:open="dialogOpen" :proxy="dialogProxy" />
    <ConfirmDialog
      v-model:open="del.open.value"
      :title="t('proxies.deleteTitle', { address: del.shown.value ? proxyAddress(del.shown.value) : '' })"
      :description="deleteText"
      :confirm-label="t('common.delete')"
      :busy="del.busy.value"
      danger
      @confirm="del.confirm"
    />
  </main>
</template>

<style scoped>
.proxies-page {
  overflow: hidden;
}

.head-actions {
  display: flex;
  gap: 10px;
}

.layout {
  display: flex;
  gap: 24px;
  flex-grow: 1;
  min-height: 0;
}

.table {
  display: flex;
  flex-direction: column;
  flex-grow: 1;
  align-self: flex-start;
  min-width: 0;
  max-height: 100%;
  overflow: hidden;
}
</style>
