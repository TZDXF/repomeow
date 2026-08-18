<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";

/**
 * 合并/变基操作流程图解(确认对话框内):
 * - merge: 源分支从分叉点拉出,末端曲线合回目标分支,产生一个新的合并提交;
 *   squash 时仅改操作标注(不自动提交的语义由开关文案说明)。
 * - rebase: 上下两段对比——变基前源分支从旧分叉点拉出,变基后源分支的提交被
 *   摘取、以虚线空心点重放到目标分支最新提交之后。
 * 画布几何固定,分支名截断后作为 SVG 文本标注,不随容器缩放。
 */
const { t } = useI18n();
const props = defineProps<{
  kind: "merge" | "rebase";
  /** 源分支名(worktree 检出的分支) */
  source: string;
  /** 目标分支名(主工作区当前分支) */
  target: string;
  squash?: boolean;
}>();

/** 图上分支名截断,避免长分支名相互遮挡 */
function short(name: string) {
  return name.length > 16 ? name.slice(0, 15) + "…" : name;
}

const mergeLabel = computed(() =>
  t(props.squash ? "git.worktree.squashAction" : "git.worktree.mergeAction"),
);
</script>

<template>
  <!-- 合并:源分支合回目标分支 -->
  <svg
    v-if="kind === 'merge'"
    viewBox="0 0 380 100"
    class="w-full rounded-md border bg-muted/30"
    fill="none"
    aria-hidden="true"
  >
    <!-- 目标分支(当前分支)干线 -->
    <g class="text-primary">
      <line x1="16" y1="62" x2="350" y2="62" stroke="currentColor" stroke-width="2" />
      <circle
        cx="44"
        cy="62"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle
        cx="96"
        cy="62"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle
        cx="150"
        cy="62"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <!-- 合并产生的新提交:实心 + 虚线外圈 -->
      <circle cx="316" cy="62" r="5" fill="currentColor" stroke="none" />
      <circle
        cx="316"
        cy="62"
        r="8.5"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-dasharray="3 3"
      />
      <path d="M 344 57 L 354 62 L 344 67 Z" fill="currentColor" stroke="none" />
    </g>
    <!-- 源分支(worktree 检出的分支):从分叉点拉出,末端合回 -->
    <g class="text-amber-500">
      <path
        d="M 96 62 C 132 62 132 30 162 30 L 268 30"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
      />
      <circle
        cx="176"
        cy="30"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle
        cx="222"
        cy="30"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle cx="268" cy="30" r="4" fill="currentColor" stroke="none" />
      <path d="M 268 30 C 300 30 300 62 304 62" stroke="currentColor" stroke-width="2" />
      <path d="M 302 57 L 311 62 L 302 67 Z" fill="currentColor" stroke="none" />
    </g>
    <!-- 标注 -->
    <text x="264" y="18" text-anchor="end" class="fill-amber-600 text-[11px] dark:fill-amber-400">
      {{ short(source) }}
    </text>
    <text x="350" y="88" text-anchor="end" class="fill-primary text-[11px]">
      {{ short(target) }}
    </text>
    <text x="312" y="42" text-anchor="start" class="fill-muted-foreground text-[10px]">
      {{ mergeLabel }}
    </text>
  </svg>

  <!-- 变基:源分支提交重放到目标分支最新提交之后 -->
  <svg
    v-else
    viewBox="0 0 380 160"
    class="w-full rounded-md border bg-muted/30"
    fill="none"
    aria-hidden="true"
  >
    <!-- 变基前 -->
    <text x="4" y="16" class="fill-muted-foreground text-[10px]">
      {{ t("git.worktree.rebaseBefore") }}
    </text>
    <g class="text-primary">
      <line x1="16" y1="36" x2="148" y2="36" stroke="currentColor" stroke-width="2" />
      <circle
        cx="40"
        cy="36"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle
        cx="90"
        cy="36"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle cx="140" cy="36" r="4" fill="currentColor" stroke="none" />
    </g>
    <g class="text-amber-500">
      <path
        d="M 90 36 C 118 36 118 60 146 60 L 226 60"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
      />
      <circle
        cx="158"
        cy="60"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle
        cx="202"
        cy="60"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle cx="226" cy="60" r="4" fill="currentColor" stroke="none" />
    </g>
    <text x="156" y="30" class="fill-primary text-[11px]">{{ short(target) }}</text>
    <text x="234" y="64" class="fill-amber-600 text-[11px] dark:fill-amber-400">
      {{ short(source) }}
    </text>

    <!-- 过渡箭头 -->
    <g class="text-muted-foreground">
      <line x1="190" y1="74" x2="190" y2="88" stroke="currentColor" stroke-width="1.5" />
      <path d="M 185 85 L 190 91 L 195 85 Z" fill="currentColor" stroke="none" />
    </g>

    <!-- 变基后:源分支提交以虚线空心点重放到目标分支 tip 之后 -->
    <text x="4" y="110" class="fill-muted-foreground text-[10px]">
      {{ t("git.worktree.rebaseAfter") }}
    </text>
    <g class="text-primary">
      <line x1="16" y1="128" x2="148" y2="128" stroke="currentColor" stroke-width="2" />
      <circle
        cx="40"
        cy="128"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle
        cx="90"
        cy="128"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
      />
      <circle cx="140" cy="128" r="4" fill="currentColor" stroke="none" />
    </g>
    <g class="text-amber-500">
      <path
        d="M 140 128 C 168 128 168 148 196 148 L 280 148"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
      />
      <circle
        cx="208"
        cy="148"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
        stroke-dasharray="2 2"
      />
      <circle
        cx="252"
        cy="148"
        r="4"
        class="fill-background"
        stroke="currentColor"
        stroke-width="2"
        stroke-dasharray="2 2"
      />
      <circle cx="280" cy="148" r="4" fill="currentColor" stroke="none" />
    </g>
    <text x="156" y="122" class="fill-primary text-[11px]">{{ short(target) }}</text>
    <text x="288" y="152" class="fill-amber-600 text-[11px] dark:fill-amber-400">
      {{ short(source) }}
    </text>
  </svg>
</template>
