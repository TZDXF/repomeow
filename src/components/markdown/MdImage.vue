<script setup lang="ts">
import { computed, inject } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { MarkdownComponentProps } from "vue-stream-markdown";
import { hasScheme, resolvePath } from "@/lib/markdown";
import { MD_BASE_PATH_KEY } from "@/components/markdown/keys";

// vue-stream-markdown 2.x 的自定义组件按标签名接管:img 节点的属性以 props 透传
const props = defineProps<
  MarkdownComponentProps & { src?: string; alt?: string; title?: string }
>();

// 相对路径解析基准目录(MD 文件所在目录),由使用方 provide
const getBasePath = inject(MD_BASE_PATH_KEY, () => "");

/** 相对路径图片换成本地 asset 协议地址,其余(http/asset/data 等)原样输出 */
const src = computed(() => {
  const url = props.src ?? "";
  if (!url || hasScheme(url)) return url;
  return convertFileSrc(resolvePath(getBasePath(), url));
});
</script>

<template>
  <img data-md-image :src="src" :alt="props.alt ?? ''" :title="props.title || undefined" />
</template>
