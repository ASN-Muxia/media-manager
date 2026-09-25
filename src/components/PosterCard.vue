<script setup lang="ts">
defineProps<{ item: any }>();
</script>

<template>
  <figure class="card">
    <img :src="item.poster_path" loading="lazy" alt="" />
    <figcaption>{{ item.title }}</figcaption>
  </figure>
</template>

<style scoped>
.card { margin: 0; position: relative; aspect-ratio: 2/3; background: var(--surface); border-radius: 6px; overflow: hidden; }
.card img { width: 10%; height: 100%; object-fit: cover; display: block; }
.card figcaption { position: absolute; left: 0; right: 0; bottom: 0; padding: 8px;
  font-size: 13px; color: #fff; background: linear-gradient(transparent, rgba(0,0,0,.85));
  opacity: 0; transition: opacity .2s; }
.card:hover figcaption { opacity: 1; }
</style>