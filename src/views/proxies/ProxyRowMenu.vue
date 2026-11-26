<script setup lang="ts">
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from 'reka-ui'
import { useI18n } from 'vue-i18n'
import UiButton from '../../components/UiButton.vue'

defineProps<{ checking: boolean }>()
const emit = defineEmits<{ check: []; edit: []; remove: [] }>()
const { t } = useI18n()
</script>

<template>
  <DropdownMenuRoot>
    <DropdownMenuTrigger as-child>
      <UiButton size="icon" variant="ghost" icon="more" :aria-label="t('common.moreActions')" :title="t('common.moreActions')" />
    </DropdownMenuTrigger>
    <DropdownMenuPortal>
      <DropdownMenuContent align="end" :side-offset="4" class="popover">
        <DropdownMenuItem class="menu-item" :disabled="checking" @select="emit('check')">
          {{ t('proxies.check') }}
        </DropdownMenuItem>
        <DropdownMenuItem class="menu-item" @select="emit('edit')">{{ t('common.edit') }}</DropdownMenuItem>
        <DropdownMenuSeparator class="separator" />
        <DropdownMenuItem class="menu-item danger" @select="emit('remove')">{{ t('common.delete') }}</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenuPortal>
  </DropdownMenuRoot>
</template>
