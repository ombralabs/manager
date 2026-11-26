<script setup lang="ts">
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuRoot, DropdownMenuTrigger } from 'reka-ui'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { core } from '../../api/core'
import UiButton from '../../components/UiButton.vue'
import { showBulkReport } from '../../lib/bulkReport'
import { notify } from '../../lib/notify'
import { useProfilesStore } from '../../stores/profiles'
import CsvExportDialog from './CsvExportDialog.vue'

const { t } = useI18n()
const profiles = useProfilesStore()
const exportOpen = ref(false)
const arranging = ref(false)

async function arrange() {
  arranging.value = true
  try {
    showBulkReport('arrange', await core.arrangeWindows())
  } catch (e) {
    notify.error(e)
  } finally {
    arranging.value = false
  }
}
</script>

<template>
  <DropdownMenuRoot>
    <DropdownMenuTrigger as-child>
      <UiButton size="icon" icon="more" :aria-label="t('common.moreActions')" :title="t('common.moreActions')" />
    </DropdownMenuTrigger>
    <DropdownMenuPortal>
      <DropdownMenuContent align="end" :side-offset="4" class="popover">
        <DropdownMenuItem
          class="menu-item"
          :disabled="profiles.only !== null"
          :title="profiles.only !== null ? t('profiles.headerMenu.exportOnlyHint') : undefined"
          @select="exportOpen = true"
        >{{ t('profiles.headerMenu.exportCsv') }}</DropdownMenuItem>
        <div v-if="profiles.only !== null" class="menu-note hint" role="presentation">{{ t('profiles.headerMenu.exportOnlyHint') }}</div>
        <DropdownMenuItem class="menu-item" :disabled="profiles.running === 0 || arranging" @select="arrange">
          {{ t('profiles.headerMenu.arrange') }}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenuPortal>
  </DropdownMenuRoot>
  <CsvExportDialog v-model:open="exportOpen" />
</template>
