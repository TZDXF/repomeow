<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { FileCode, LoaderCircle } from "@lucide/vue";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import CodeViewer from "@/components/files/CodeViewer.vue";
import { cmd } from "@/lib/tauri";
import type { FilePreview } from "@/types";

/**
 * wiki 页面的来源文件查看对话框:点击页面底部来源文件 chips 打开,
 * 经 read_file_preview 读取(二进制返回 text=null,超 512KB 截断)
 */
const props = defineProps<{
  /** 项目根目录 */
  root: string;
  /** 要查看的仓库内相对路径;null 表示关闭 */
  relPath: string | null;
}>();
const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();

const open = computed({
  get: () => props.relPath !== null,
  set: (v: boolean) => {
    if (!v) emit("close");
  },
});

const preview = ref<FilePreview | null>(null);
const loading = ref(false);
let loadSeq = 0;

watch(
  () => props.relPath,
  async (rel) => {
    if (!rel) {
      preview.value = null;
      return;
    }
    const seq = ++loadSeq;
    loading.value = true;
    try {
      const result = await cmd<FilePreview>("read_file_preview", {
        root: props.root,
        relPath: rel,
      });
      if (seq === loadSeq) preview.value = result;
    } catch {
      if (seq === loadSeq) preview.value = null;
    } finally {
      if (seq === loadSeq) loading.value = false;
    }
  },
);
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="flex max-h-[85vh] flex-col sm:max-w-3xl">
      <DialogHeader class="shrink-0">
        <DialogTitle class="flex items-center gap-2 pr-8 text-sm">
          <FileCode class="h-4 w-4 shrink-0 text-muted-foreground" />
          <span class="truncate font-mono text-xs">{{ relPath }}</span>
        </DialogTitle>
      </DialogHeader>
      <div v-if="loading" class="flex flex-1 items-center justify-center py-12">
        <LoaderCircle class="h-5 w-5 animate-spin text-muted-foreground" />
      </div>
      <p
        v-else-if="!preview || preview.text === null"
        class="py-12 text-center text-sm text-muted-foreground"
      >
        {{ t("files.binary") }}
      </p>
      <!-- CodeViewer 宿主需要确定高度(CM 自带 scroller) -->
      <div v-else class="flex h-[65vh] flex-col">
        <p v-if="preview.truncated" class="shrink-0 pb-1 text-xs text-muted-foreground">
          {{ t("files.truncated") }}
        </p>
        <div class="min-h-0 flex-1">
          <CodeViewer :text="preview.text" :path="relPath ?? ''" :wrap="false" />
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
