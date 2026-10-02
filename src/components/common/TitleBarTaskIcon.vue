<script setup lang="ts">
import type { Component } from "vue";
import { LoaderCircle } from "@lucide/vue";

/**
 * 标题栏任务入口图标(后台任务/终端会话共用):
 * 左侧始终显示类型图标以便区分;running 时右侧依次跟旋转 spinner 与数量,
 * 均为主题 primary 色。
 */
defineProps<{
  icon: Component;
  running: boolean;
  count: number;
}>();
</script>

<template>
  <span class="flex items-center gap-1">
    <component :is="icon" class="h-4 w-4 shrink-0" />
    <template v-if="running">
      <LoaderCircle class="h-3 w-3 shrink-0 animate-spin text-primary motion-reduce:animate-none" />
      <span class="shrink-0 text-xs font-semibold tabular-nums text-primary">{{ count }}</span>
    </template>
  </span>
</template>
