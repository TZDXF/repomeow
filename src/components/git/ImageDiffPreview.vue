<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { Loader2 } from "@lucide/vue";
import ImageViewer from "@/components/files/ImageViewer.vue";
import { baseName } from "@/lib/path";

/**
 * 提交详情面板的图片 diff 预览:二进制图片没有文本 diff 可渲染,改为直接展示
 * 新旧版本图像(新增只看新、删除只看旧、修改/重命名左右对照),每栏复用
 * ProjectFiles 的 ImageViewer(滚轮缩放/拖拽平移/尺寸标注),与文件页观感一致。
 * 取数(blob → data URL)由父组件 CommitDetailPanel 负责。
 */
defineProps<{
  /** 当前文件路径(标题栏) */
  filePath: string | null;
  loading: boolean;
  error: string;
  /** 预览栏(旧/新);src 为 data URL,path 用于栏头文件名与悬浮提示 */
  panes: { key: string; label: string; path: string; src: string; svg: boolean }[];
}>();

const { t } = useI18n();
</script>

<template>
  <div class="flex min-h-0 min-w-0 flex-1 flex-col">
    <div class="flex shrink-0 items-center gap-2 border-b px-3 py-1.5">
      <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="filePath ?? undefined">
        {{ filePath ?? "" }}
      </span>
    </div>

    <div v-if="loading" class="flex min-h-0 flex-1 items-center justify-center">
      <Loader2 class="h-4 w-4 animate-spin text-muted-foreground" />
    </div>
    <p v-else-if="error" class="px-3 py-2 text-xs text-destructive">
      {{ t("git.graph.detail.diffLoadFailed") }}:{{ error }}
    </p>

    <div v-else class="flex min-h-0 flex-1">
      <div
        v-for="pane in panes"
        :key="pane.key"
        class="flex min-h-0 min-w-0 flex-1 flex-col border-r last:border-r-0"
      >
        <div
          class="flex shrink-0 items-center gap-2 border-b bg-muted/40 px-3 py-1 text-xs text-muted-foreground"
        >
          <span class="shrink-0 font-medium">{{ pane.label }}</span>
          <span class="min-w-0 flex-1 truncate text-right font-mono" :title="pane.path">
            {{ baseName(pane.path) }}
          </span>
        </div>
        <!-- ImageViewer 根节点自带 h-full,外包一层撑满剩余高度 -->
        <div class="min-h-0 flex-1">
          <ImageViewer :src="pane.src" :svg="pane.svg" :alt="pane.path" />
        </div>
      </div>
    </div>
  </div>
</template>
