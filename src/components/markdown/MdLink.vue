<script setup lang="ts">
import { computed } from "vue";
import { NodeList, type LinkNodeRendererProps } from "vue-stream-markdown";
import { safeLinkHref } from "@/lib/markdown";

// 自定义链接渲染器:href 先过协议白名单(http/https/mailto、# 锚点、相对/本地路径),
// 危险协议(javascript:/data:/file: 等)降级为纯文本,中键/右键等路径也触达不到原始 href;
// 点击行为由外层统一拦截(外链交系统浏览器,相对路径解析后打开;同 ProjectFiles.onBodyClick)
const props = defineProps<LinkNodeRendererProps>();
const safeHref = computed(() => safeLinkHref(props.node.url));
</script>

<template>
  <a v-if="safeHref !== null" data-md-link :href="safeHref" rel="noreferrer">
    <NodeList
      v-bind="props"
      :parent-node="props.node"
      :nodes="props.node.children"
      :deep="props.deep + 1"
    />
  </a>
  <span v-else data-md-link-disabled>
    <NodeList
      v-bind="props"
      :parent-node="props.node"
      :nodes="props.node.children"
      :deep="props.deep + 1"
    />
  </span>
</template>
