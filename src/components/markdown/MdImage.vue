<script setup lang="ts">
import { inject } from "vue";
import type { MarkdownComponentProps } from "vue-stream-markdown";
import { useMarkdownImage } from "./useMarkdownImage";
import { MD_BASE_PATH_KEY, MD_ROOT_PATH_KEY } from "@/components/markdown/keys";

// vue-stream-markdown 2.x 的自定义组件按标签名接管:img 节点的属性以 props 透传
const props = defineProps<
  MarkdownComponentProps & { src?: string; alt?: string; title?: string }
>();

// 相对路径解析基准目录(MD 文件所在目录),由使用方 provide
const getBasePath = inject(MD_BASE_PATH_KEY, () => "");

const getRootPath = inject(MD_ROOT_PATH_KEY, () => "");

const src = useMarkdownImage(() => props.src ?? "", getBasePath, getRootPath);
</script>

<template>
  <img
    v-if="src"
    data-md-image
    :src="src"
    :alt="props.alt ?? ''"
    :title="props.title || undefined"
  />
</template>
