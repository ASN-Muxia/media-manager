<script setup lang="ts">
import PosterCard from "./PosterCard.vue";
defineProps<{ items: any[] }>();
</script>

<template>
  <div class="grid">
    <PosterCard v-for="it in items" :key="it.id" :item="it" />
  </div>
</template>

<style scoped>
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: calc(var(--space) * 4); }
</style>