<script setup lang="ts">
import { ref, watchEffect } from "vue";
import { useI18n } from "vue-i18n";
import { useLocalStorage } from "@vueuse/core";
import {
  ChevronDown,
  ChevronUp,
  Columns2,
  Eraser,
  ExternalLink,
  Highlighter,
  Loader2,
  Rows2,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import PierreDiff from "@/components/pierre/PierreDiff.vue";
import type { GitCommitFileDiff } from "@/types";

/**
 * 单文件 diff 查看器:提交详情面板与提交对话框变更预览共用。
 * 本组件只是工具条壳(文件路径 / 截断徽标 / 差异导航 / 忽略空白 / 词级高亮 / 并排切换 / IDE 打开),
 * 解析、着色、折叠、并排渲染全部交给 @pierre/diffs(PierreDiff 包装);
 * 只接收「取数结果」,取数本身由父组件负责(提交 diff 与工作区 diff 命令不同)。
 */
const props = defineProps<{
  /** 当前文件的 diff 结果;null = 尚未加载 */
  diff: GitCommitFileDiff | null;
  /** diff 对应的文件路径(标题与着色语言推断);切文件时先于 diff 变化 */
  filePath: string | null;
  loading: boolean;
  error: string;
  /** 并排是否适用:新增/删除文件一侧必然全空,强制逐行视图并隐藏切换按钮 */
  splitApplicable: boolean;
  /** 当前文件是否可在 IDE 打开(已删除文件工作区已不存在,不可打开) */
  canOpenIde: boolean;
}>();

/** 忽略空白差异模式:none 不忽略 / eol 行尾 / change 空白数量变化 / all 全部空白。
 *  由父组件持有(持久化),模式变化需父组件按新模式重取 diff(行集会变) */
const ignoreWs = defineModel<"none" | "eol" | "change" | "all">("ignoreWs", { required: true });

const emit = defineEmits<{
  /** 在 IDE 打开当前文件(路径拼接与编辑器选择由父组件负责) */
  openIde: [];
}>();

const { t } = useI18n();

/** 行内词级差异高亮(持久化) */
const wordDiff = useLocalStorage("repomeow:commit-diff-word", true);
/** 并排查看(持久化):旧版本在左、新版本在右 */
const splitDiff = useLocalStorage("repomeow:commit-diff-split", false);

const pierreDiff = ref<InstanceType<typeof PierreDiff> | null>(null);
/** 差异导航按钮禁用态:跟踪 PierreDiff 暴露的响应式状态(渲染完成/滚动时其内部刷新) */
const hasPrevChange = ref(false);
const hasNextChange = ref(false);

watchEffect(() => {
  hasPrevChange.value = pierreDiff.value?.hasPrevChange ?? false;
  hasNextChange.value = pierreDiff.value?.hasNextChange ?? false;
});

function stepChange(dir: 1 | -1) {
  pierreDiff.value?.stepChange(dir);
}
</script>

<template>
  <div class="commit-diff flex min-h-0 min-w-0 flex-1 flex-col">
    <div class="flex shrink-0 items-center gap-2 border-b px-3 py-1.5">
      <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="filePath ?? undefined">
        {{ filePath ?? "" }}
      </span>
      <Badge v-if="diff?.truncated" variant="outline" class="h-5 shrink-0 px-1.5 text-[10px]">
        {{ t("git.graph.detail.diffTruncated") }}
      </Badge>
      <template v-if="diff && filePath">
        <button
          class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors not-disabled:hover:bg-accent not-disabled:hover:text-foreground disabled:opacity-40"
          :disabled="!hasPrevChange"
          :title="t('git.graph.detail.diffPrevChange')"
          @click="stepChange(-1)"
        >
          <ChevronUp class="h-3.5 w-3.5" />
        </button>
        <button
          class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors not-disabled:hover:bg-accent not-disabled:hover:text-foreground disabled:opacity-40"
          :disabled="!hasNextChange"
          :title="t('git.graph.detail.diffNextChange')"
          @click="stepChange(1)"
        >
          <ChevronDown class="h-3.5 w-3.5" />
        </button>
      </template>
      <DropdownMenu v-if="diff">
        <DropdownMenuTrigger as-child>
          <button
            class="shrink-0 rounded-sm p-1 transition-colors hover:bg-accent hover:text-foreground"
            :class="ignoreWs !== 'none' ? 'bg-accent text-foreground' : 'text-muted-foreground'"
            :title="t('git.graph.detail.diffIgnoreWs')"
          >
            <Eraser class="h-3.5 w-3.5" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-auto whitespace-nowrap">
          <DropdownMenuRadioGroup v-model="ignoreWs">
            <DropdownMenuRadioItem value="none">
              {{ t("git.graph.detail.diffIgnoreWsNone") }}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="eol">
              {{ t("git.graph.detail.diffIgnoreWsEol") }}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="change">
              {{ t("git.graph.detail.diffIgnoreWsChange") }}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="all">
              {{ t("git.graph.detail.diffIgnoreWsAll") }}
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <button
        v-if="diff"
        class="shrink-0 rounded-sm p-1 transition-colors hover:bg-accent hover:text-foreground"
        :class="wordDiff ? 'bg-accent text-foreground' : 'text-muted-foreground'"
        :title="t('git.graph.detail.diffWordHl')"
        @click="wordDiff = !wordDiff"
      >
        <Highlighter class="h-3.5 w-3.5" />
      </button>
      <button
        v-if="diff && splitApplicable"
        class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="t(splitDiff ? 'git.graph.detail.diffUnified' : 'git.graph.detail.diffSplit')"
        @click="splitDiff = !splitDiff"
      >
        <Rows2 v-if="splitDiff" class="h-3.5 w-3.5" />
        <Columns2 v-else class="h-3.5 w-3.5" />
      </button>
      <button
        v-if="canOpenIde"
        class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="t('git.graph.detail.openInIde')"
        @click="emit('openIde')"
      >
        <ExternalLink class="h-3.5 w-3.5" />
      </button>
    </div>

    <div v-if="loading" class="flex min-h-0 flex-1 items-center justify-center">
      <Loader2 class="h-4 w-4 animate-spin text-muted-foreground" />
    </div>
    <p v-else-if="error" class="px-3 py-2 text-xs text-destructive">
      {{ t("git.graph.detail.diffLoadFailed") }}:{{ error }}
    </p>
    <p
      v-else-if="!filePath"
      class="flex min-h-0 flex-1 items-center justify-center text-xs text-muted-foreground"
    >
      {{ t("git.graph.detail.selectFile") }}
    </p>
    <!-- 旧 diff 保留到新结果落地由父组件数据流保证(取数期间不清空 diff),patch 变化经 PierreDiff 内部重放 -->
    <PierreDiff
      v-else-if="diff"
      ref="pierreDiff"
      :patch="diff.diff"
      :file-path="filePath"
      :split="splitDiff && splitApplicable"
      :word-diff="wordDiff"
      :truncated="diff.truncated"
    />
    <div v-else class="min-h-0 flex-1" />
  </div>
</template>
