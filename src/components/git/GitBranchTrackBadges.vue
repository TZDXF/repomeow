<script setup lang="ts">
import { useI18n } from "vue-i18n";

/** 本地分支与其 upstream 的 ahead/behind 徽标:behind>0 显示 n↓、ahead>0 显示 n↑ */
defineProps<{ ahead: number; behind: number }>();

const { t } = useI18n();
</script>

<template>
  <span
    v-if="ahead > 0 || behind > 0"
    class="flex shrink-0 items-center gap-1 text-[10px] font-medium"
  >
    <span
      v-if="ahead > 0"
      class="text-emerald-600"
      :title="t('git.branch.aheadCount', { n: ahead })"
    >
      {{ ahead }}↑
    </span>
    <span
      v-if="behind > 0"
      class="text-amber-600"
      :title="t('git.branch.behindCount', { n: behind })"
    >
      {{ behind }}↓
    </span>
  </span>
</template>
