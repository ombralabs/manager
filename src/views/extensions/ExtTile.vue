<script setup lang="ts">
import { computed } from 'vue'
import { tileColor, tileLetter } from '../../lib/extensions'

/** Плитка расширения с холста: первая буква имени на цветном квадрате; цвет — по `id`, у расширения он не меняется. */
const props = withDefaults(defineProps<{ id: string; name: string; size?: 36 | 48 }>(), { size: 48 })
const style = computed(() => ({
  width: `${props.size}px`,
  height: `${props.size}px`,
  borderRadius: props.size === 48 ? '12px' : '8px',
  fontSize: props.size === 48 ? '22px' : '15px',
  background: tileColor(props.id),
}))
</script>

<template>
  <div class="tile" aria-hidden="true" :style="style">{{ tileLetter(name) }}</div>
</template>

<style scoped>
.tile {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: #fff;
  font-weight: 600;
  line-height: 1;
}
</style>
