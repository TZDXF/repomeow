<script setup lang="ts">
import { computed, inject } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { ImageNodeRendererProps } from "vue-stream-markdown";
import { hasScheme, resolvePath } from "@/lib/markdown";
import { MD_BASE_PATH_KEY } from "@/components/markdown/keys";

const props = defineProps<ImageNodeRendererProps>();

// 相对路径解析基准目录(MD 文件所在目录),由使用方 provide
const getBasePath = inject(MD_BASE_PATH_KEY, () => "");

/** 相对路径图片换成本地 asset 协议地址,其余(http/asset/data 等)原样输出 */
const src = computed(() => {
  const url = props.node.url ?? "";
  if (!url || hasScheme(url)) return url;
  return convertFileSrc(resolvePath(getBasePath(), url));
});

const alt = computed(() => String(props.node.alt ?? ""));
const title = computed(() => String(props.node.title ?? ""));
</script>

<template>
  <img data-md-image :src="src" :alt="alt" :title="title || undefined" />
</template>
