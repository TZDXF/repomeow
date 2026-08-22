<script setup lang="ts">
import { NodeList, type LinkNodeRendererProps } from "vue-stream-markdown";

// 自定义链接渲染器:输出真实 href,点击行为由外层统一拦截
// (外链交系统浏览器,相对路径用系统默认程序打开;同 ProjectFiles.onBodyClick)
const props = defineProps<LinkNodeRendererProps>();
</script>

<template>
  <a data-md-link :href="props.node.url ?? ''" rel="noreferrer">
    <NodeList
      v-bind="props"
      :parent-node="props.node"
      :nodes="props.node.children"
      :deep="props.deep + 1"
    />
  </a>
</template>
