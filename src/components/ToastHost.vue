<script setup lang="ts">
import { ToastClose, ToastDescription, ToastProvider, ToastRoot, ToastTitle, ToastViewport } from 'reka-ui'
import { useI18n } from 'vue-i18n'
import { useToasts } from '../stores/toasts'
import AppIcon from './AppIcon.vue'
import UiDetails from './UiDetails.vue'

const toasts = useToasts()
const { t } = useI18n()

function onOpenChange(id: number, open: boolean) {
  if (!open) toasts.dismiss(id)
}
</script>

<template>
  <ToastProvider :label="t('common.notification')">
    <ToastRoot
      v-for="toast in toasts.items"
      :key="toast.id"
      :open="true"
      :duration="toast.kind === 'error' ? 10000 : 6000"
      :class="['ombra-toast', toast.kind]"
      @update:open="onOpenChange(toast.id, $event)"
    >
      <div class="text">
        <ToastTitle class="title">{{ toast.title }}</ToastTitle>
        <ToastDescription v-if="toast.body" class="hint body">{{ toast.body }}</ToastDescription>
        <UiDetails v-if="toast.details" :text="toast.details" />
        <RouterLink v-if="toast.action" :to="toast.action.to" class="toast-action" @click="toasts.dismiss(toast.id)">{{ toast.action.label }}</RouterLink>
      </div>
      <ToastClose :aria-label="t('common.close')" class="close">
        <AppIcon name="close" :size="16" />
      </ToastClose>
    </ToastRoot>
    <ToastViewport class="toast-viewport" />
  </ToastProvider>
</template>

<style scoped>
.text {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex-grow: 1;
  min-width: 0;
}

.title {
  font-weight: 600;
  overflow-wrap: anywhere;
}

.toast-action { align-self: flex-start; display: inline-flex; align-items: center; min-height: 44px; color: var(--link); text-underline-offset: 3px; }
.toast-action:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }

/* Ошибка проверки — по строке на поле. */
.body {
  white-space: pre-line;
}

.close {
  padding: 2px;
  border: 0;
  background: transparent;
  color: var(--text-2);
  cursor: pointer;
}
</style>

<style>
/*
 * Корни reka-ui не получают атрибут области `data-v-…`: у `ToastViewport` `inheritAttrs: false` (класс уходит
 * на внутренний `ol`), `ToastRoot` рисуется через `Teleport` в этот `ol`. Поэтому их правила — без `scoped`,
 * с именами, которых нет у других компонентов (как `.popover` в base.css).
 */
.toast-viewport {
  position: fixed;
  right: 24px;
  bottom: 24px;
  z-index: 60;
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: min(380px, calc(100vw - 48px));
  margin: 0;
  padding: 0;
  list-style: none;
  outline: none;
}

.ombra-toast {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 14px 16px;
  border: 1px solid var(--border);
  border-left-width: 4px;
  border-radius: var(--radius-control);
  background: var(--surface);
  box-shadow: 0 8px 24px color-mix(in srgb, var(--text) 14%, transparent);
}

.ombra-toast.error {
  border-left-color: var(--danger-text);
}

.ombra-toast.warning {
  border-left-color: var(--warn-text);
}

.ombra-toast.info {
  border-left-color: var(--text-2);
}
</style>
