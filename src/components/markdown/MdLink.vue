<script setup lang="ts">
import { computed } from "vue";
import type { MarkdownComponentProps } from "vue-stream-markdown";
import { safeLinkHref } from "@/lib/markdown";

// vue-stream-markdown 2.x 的自定义组件按标签名接管:a 节点的属性以 props 透传,
// 子节点走默认 slot。href 先过协议白名单(http/https/mailto、# 锚点、相对/本地
// 路径),危险协议(javascript:/data:/file: 等)降级为纯文本,中键/右键等路径也
// 触达不到原始 href;点击行为由外层统一拦截(外链交系统浏览器,相对路径解析后
// 打开;同 ProjectFiles.onBodyClick)
const props = defineProps<MarkdownComponentProps & { href?: string }>();

const safeHref = computed(() => safeLinkHref(props.href ?? null));
</script>

<template>
  <a v-if="safeHref !== null" data-md-link :href="safeHref" rel="noreferrer">
    <slot />
  </a>
  <span v-else data-md-link-disabled>
    <slot />
  </span>
</template>
