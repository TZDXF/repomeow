<script setup lang="ts">
import { onMounted, ref } from "vue";
import type { Component } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getEditorIcons } from "@/lib/open-with";
import type { EditorKind } from "@/types";

// 内置打开方式优先展示本机真实图标(后端从 exe / .app 提取缓存);
// 自定义方式没有本机程序绑定,始终展示用户选择的 Lucide 图标。
const props = defineProps<{ kind?: EditorKind; icon: Component; iconClass?: string }>();

const url = ref<string | null>(null);
const broken = ref(false);

onMounted(async () => {
  if (!props.kind) return;
  const icons = await getEditorIcons();
  const path = icons[props.kind];
  if (path) {
    url.value = convertFileSrc(path);
  }
});
</script>

<template>
  <img
    v-if="url && !broken"
    :src="url"
    :class="iconClass"
    alt=""
    draggable="false"
    @error="broken = true"
  />
  <component :is="icon" v-else :class="iconClass" />
</template>
