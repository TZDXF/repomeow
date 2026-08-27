<script setup lang="ts">
/** 分析面板卡片的统一容器:标题 + 可选右侧提示 + 内容区 */
defineProps<{
  title: string;
  hint?: string;
}>();
</script>

<template>
  <section class="rounded-lg border p-4">
    <div class="flex items-baseline justify-between gap-2">
      <h3 class="text-sm font-medium">{{ title }}</h3>
      <p v-if="hint" class="shrink-0 text-xs text-muted-foreground">{{ hint }}</p>
    </div>
    <div class="mt-3">
      <slot />
    </div>
  </section>
</template>
