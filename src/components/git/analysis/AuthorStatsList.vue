<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { formatCompactNumber, formatRelativeTime } from "@/lib/format";
import { topWithOther } from "@/lib/git-stats";
import type { GitAuthorStat } from "@/types";

const props = defineProps<{
  authors: GitAuthorStat[];
}>();

const { t } = useI18n();

const TOP_AUTHORS = 10;

interface AuthorRow {
  key: string;
  name: string;
  email: string;
  commits: number;
  additions: number;
  deletions: number;
  /** null = 「其他」归并行,不显示活跃时间 */
  lastCommitAt: number | null;
  other: boolean;
}

const rows = computed<AuthorRow[]>(() => {
  const toRow = (a: GitAuthorStat): AuthorRow => ({
    key: a.email || `name:${a.name}`,
    name: a.name || a.email,
    email: a.email,
    commits: a.commits,
    additions: a.additions,
    deletions: a.deletions,
    lastCommitAt: a.lastCommitAt,
    other: false,
  });
  return topWithOther(props.authors.map(toRow), TOP_AUTHORS, (rest) => ({
    key: "__other__",
    name: t("git.graph.analysis.otherAuthors", { count: rest.length }),
    email: "",
    commits: rest.reduce((s, a) => s + a.commits, 0),
    additions: rest.reduce((s, a) => s + a.additions, 0),
    deletions: rest.reduce((s, a) => s + a.deletions, 0),
    lastCommitAt: null,
    other: true,
  }));
});

const maxCommits = computed(() => Math.max(...rows.value.map((r) => r.commits), 1));

/** 头像底色按行序轮换(确定性的柔和色板,亮暗主题均可读) */
const AVATAR_COLORS = [
  "bg-blue-500/15 text-blue-600 dark:text-blue-400",
  "bg-violet-500/15 text-violet-600 dark:text-violet-400",
  "bg-amber-500/15 text-amber-600 dark:text-amber-400",
  "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
  "bg-rose-500/15 text-rose-600 dark:text-rose-400",
  "bg-cyan-500/15 text-cyan-600 dark:text-cyan-400",
] as const;

function avatarClass(index: number): string {
  return AVATAR_COLORS[index % AVATAR_COLORS.length];
}

function initial(name: string): string {
  return (name.trim()[0] ?? "?").toUpperCase();
}
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <div
      v-for="(row, index) in rows"
      :key="row.key"
      class="flex items-center gap-2.5"
      :title="row.email || undefined"
    >
      <span
        class="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-xs font-semibold"
        :class="row.other ? 'bg-muted text-muted-foreground' : avatarClass(index)"
      >
        {{ row.other ? "…" : initial(row.name) }}
      </span>
      <div class="min-w-0 flex-1">
        <div class="flex items-baseline justify-between gap-2">
          <span class="truncate text-xs font-medium">{{ row.name }}</span>
          <span class="shrink-0 text-[11px] tabular-nums text-muted-foreground">
            {{ t("git.graph.analysis.authorsCommits", { count: row.commits }) }}
            <template v-if="row.additions > 0 || row.deletions > 0">
              ·
              <span class="text-emerald-600 dark:text-emerald-400"
                >+{{ formatCompactNumber(row.additions) }}</span
              >
              <span class="text-rose-600 dark:text-rose-400"
                >/−{{ formatCompactNumber(row.deletions) }}</span
              >
            </template>
          </span>
        </div>
        <div class="mt-1 flex items-center gap-2">
          <div class="h-1.5 min-w-0 flex-1 rounded-full bg-accent">
            <div
              class="h-full rounded-full bg-primary/70"
              :style="{ width: `${(row.commits / maxCommits) * 100}%` }"
            />
          </div>
          <span class="w-16 shrink-0 text-right text-[10px] text-muted-foreground">
            {{ row.lastCommitAt ? formatRelativeTime(row.lastCommitAt) : "" }}
          </span>
        </div>
      </div>
    </div>
  </div>
</template>
