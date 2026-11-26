<script setup lang="ts">
import { computed } from 'vue'
import { opIcon } from '../lib/opIcons'

const props = withDefaults(defineProps<{ op: string; size?: number }>(), { size: 18 })
const icon = computed(() => opIcon(props.op))
</script>

<template>
  <svg :width="size" :height="size" viewBox="0 0 24 24" aria-hidden="true" class="icon op-icon"
    :data-op-icon="op" :class="{ filled: icon.filled }" :stroke-width="icon.stroke ?? 1.7" v-html="icon.body" />
</template>
