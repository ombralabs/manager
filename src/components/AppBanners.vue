<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { core } from '../api/core'
import type { Announcement } from '../api/types'
import { notify } from '../lib/notify'
import { useSecurityStore } from '../stores/security'
import AppIcon from './AppIcon.vue'
import UiButton from './UiButton.vue'

defineProps<{ securityNotice?: boolean }>()
const { t, locale } = useI18n()
const router = useRouter()
const security = useSecurityStore()
const campaigns = ref<Announcement[]>([])
const dismissed = ref(new Set<string>())
const key = (campaign: Announcement) => `ombra.announcement.${campaign.id}.${campaign.revision}`
const visible = computed(() => campaigns.value.filter((campaign) => !dismissed.value.has(key(campaign))))
onMounted(async () => {
  try {
    campaigns.value = await core.listAnnouncements()
    for (const campaign of campaigns.value) {
      try { if (campaign.dismissible && localStorage.getItem(key(campaign)) === '1') dismissed.value.add(key(campaign)) } catch { /* Настройки окна могут быть недоступны. */ }
    }
  } catch { /* Недоступность рекламного каталога не мешает работе. */ }
})
function dismiss(campaign: Announcement) {
  if (!campaign.dismissible) return
  dismissed.value.add(key(campaign))
  try { localStorage.setItem(key(campaign), '1') } catch { /* Скрываем в текущем сеансе. */ }
}
const copy = (value: { ru: string; en: string }) => locale.value === 'en' ? value.en : value.ru
</script>

<template>
  <div v-if="(securityNotice && !security.status?.configured) || visible.length" class="app-banners" :aria-label="t('announcements.label')">
    <section v-if="securityNotice && !security.status?.configured" class="app-banner security-banner" data-test="security-banner">
      <AppIcon name="shield" :size="26" />
      <div class="banner-copy"><strong>{{ t('security.bannerTitle') }}</strong><p>{{ t('security.bannerText') }}</p></div>
      <UiButton variant="soft" @click="router.push('/settings#security')">{{ t('security.configure') }}</UiButton>
    </section>
    <section v-for="campaign in visible" :key="key(campaign)" class="app-banner" :class="campaign.tone" :data-campaign="campaign.id">
      <AppIcon name="megaphone" :size="24" />
      <div class="banner-copy"><strong>{{ copy(campaign.title) }}</strong><p>{{ copy(campaign.body) }}</p></div>
      <UiButton v-if="campaign.url && campaign.action_label" variant="soft" @click="core.openAnnouncement(campaign.id).catch(notify.error)">{{ copy(campaign.action_label) }}</UiButton>
      <UiButton v-if="campaign.dismissible" size="icon" variant="ghost" icon="close" :aria-label="t('common.close')" @click="dismiss(campaign)" />
    </section>
  </div>
</template>

<style scoped>
.app-banners { display: flex; flex-direction: column; gap: 10px; flex-shrink: 0; }
.app-banner { display: flex; align-items: center; gap: 16px; padding: 18px 20px; border: 1px solid var(--border); border-radius: var(--radius-card); background: var(--muted-bg); }
.app-banner > .icon { flex-shrink: 0; color: var(--accent-ink); }
.banner-copy { min-width: 0; flex: 1; }
.banner-copy strong { font-size: 14px; font-weight: 600; }
.banner-copy p { margin: 4px 0 0; color: var(--text-2); font-size: 13px; line-height: 19px; overflow-wrap: anywhere; }
.security-banner, .app-banner.info { background: var(--accent-soft); border-color: transparent; }
.app-banner.promotion { background: color-mix(in srgb, var(--accent-soft) 45%, var(--surface)); }
@media (max-width: 800px) { .app-banner { flex-wrap: wrap; gap: 12px; padding: 16px; } .banner-copy { flex-basis: calc(100% - 48px); } }
</style>
