<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { formatBytes } from "@/lib/format";
import { topWithOther } from "@/lib/git-stats";
import type { GitFileTypeStat } from "@/types";

const props = defineProps<{
  fileTypes: GitFileTypeStat[];
  totalBytes: number;
}>();

const { t } = useI18n();

const TOP_TYPES = 8;

interface TypeRow {
  ext: string;
  files: number;
  bytes: number;
  /** 字节占比(0-1) */
  share: number;
  other: boolean;
}

const rows = computed<TypeRow[]>(() => {
  const total = Math.max(props.totalBytes, 1);
  const toRow = (f: GitFileTypeStat): TypeRow => ({
    ext: f.ext,
    files: f.files,
    bytes: f.bytes,
    share: f.bytes / total,
    other: false,
  });
  return topWithOther(props.fileTypes.map(toRow), TOP_TYPES, (rest) => ({
    ext: "__other__",
    files: rest.reduce((s, f) => s + f.files, 0),
    bytes: rest.reduce((s, f) => s + f.bytes, 0),
    share: rest.reduce((s, f) => s + f.share, 0),
    other: true,
  }));
});

/** 类型配色:按行序轮换的确定性色板(语言条与图例共用) */
const TYPE_COLORS = [
  "bg-blue-500",
  "bg-violet-500",
  "bg-amber-500",
  "bg-emerald-500",
  "bg-rose-500",
  "bg-cyan-500",
  "bg-orange-500",
  "bg-lime-600",
  "bg-slate-400",
] as const;

function colorClass(index: number): string {
  return TYPE_COLORS[index % TYPE_COLORS.length];
}

function extLabel(row: TypeRow): string {
  if (row.other) return t("git.graph.analysis.typeOther");
  // "(other)" 是后端对无扩展名文件的归并键
  if (row.ext === "(other)") return t("git.graph.analysis.extOther");
  return `.${row.ext}`;
}

function pct(share: number): string {
  return `${(share * 100).toFixed(1)}%`;
}
</script>

<template>
  <div>
    <!-- GitHub 语言条风格的整宽堆叠条 -->
    <div class="flex h-2.5 w-full gap-0.5 overflow-hidden">
      <div
        v-for="(row, index) in rows"
        :key="row.ext"
        class="h-full rounded-full"
        :class="colorClass(index)"
        :style="{ width: `${Math.max(row.share * 100, 0.8)}%` }"
        :title="`${extLabel(row)} · ${pct(row.share)}`"
      />
    </div>
    <div class="mt-3 grid grid-cols-1 gap-x-4 gap-y-1 sm:grid-cols-2">
      <div
        v-for="(row, index) in rows"
        :key="row.ext"
        class="flex items-center gap-2 text-xs"
        :title="
          t('git.graph.analysis.typeTooltip', { files: row.files, bytes: formatBytes(row.bytes) })
        "
      >
        <span class="h-2.5 w-2.5 shrink-0 rounded-[3px]" :class="colorClass(index)" />
        <span class="min-w-0 flex-1 truncate font-medium">{{ extLabel(row) }}</span>
        <span class="shrink-0 tabular-nums text-muted-foreground">{{
          formatBytes(row.bytes)
        }}</span>
        <span class="w-12 shrink-0 text-right tabular-nums text-muted-foreground">{{
          pct(row.share)
        }}</span>
      </div>
    </div>
  </div>
</template>
