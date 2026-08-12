<script setup lang="ts">
import { useI18n } from "vue-i18n";
import {
  ArrowDownToLine,
  ArrowUpToLine,
  ChevronRight,
  Folder,
  GitBranch,
  Globe,
  Loader2,
  LogIn,
  Trash2,
} from "@lucide/vue";
import {
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "@/components/ui/dropdown-menu";
import GitBranchTrackBadges from "@/components/git/GitBranchTrackBadges.vue";
import type { BranchTreeNode } from "@/lib/branch-tree";

const { t } = useI18n();
// 树结构渲染分支:目录节点内联展开/收起(chevron 切换),分支节点悬停/点击展开嵌套操作子菜单。
// 既是分支又是目录的节点(如 feature 与 feature/x 并存):行内 chevron 控制子级内联展开,
// 行本身悬停展开自身操作子菜单
const props = withDefaults(
  defineProps<{
    nodes: BranchTreeNode[];
    /** 远程分组:操作项为 签出/删除远程分支,目录顶层用 Globe 图标 */
    remote?: boolean;
    depth?: number;
    currentBranch?: string;
    /** 分支名 → upstream 跟踪差值(仅本地分支有) */
    trackByName?: Map<string, { ahead: number; behind: number }>;
    /** 进行中的分支操作(拉取/推送),用于禁用与展示 spinner */
    branchOp?: { branch: string; op: "pull" | "push" } | null;
    /** 任一 git 操作进行中(签出/拉取/推送等),锁定全部操作项 */
    locked?: boolean;
    /** 已折叠目录键集合(键见 folderKey) */
    collapsed: Set<string>;
  }>(),
  {
    remote: false,
    depth: 0,
    currentBranch: "",
    trackByName: undefined,
    branchOp: null,
    locked: false,
  },
);
const emit = defineEmits<{
  toggleFolder: [key: string];
  checkout: [branch: string, remote: boolean];
  pull: [branch: string];
  push: [branch: string];
  remove: [branch: string, remote: boolean];
}>();

/** 折叠键带分组前缀:本地目录与远程目录可能同名(如本地 feature 与 origin/feature) */
function folderKey(node: BranchTreeNode) {
  return `${props.remote ? "remote" : "local"}:${node.fullPath}`;
}

function trackOf(branch: string | null) {
  return branch ? props.trackByName?.get(branch) : undefined;
}

function opOf(branch: string | null, op: "pull" | "push") {
  return !!branch && props.branchOp?.branch === branch && props.branchOp.op === op;
}
</script>

<template>
  <template v-for="node in nodes" :key="node.fullPath">
    <!-- 分支节点:行即子菜单触发器,悬停/点击展开操作列表 -->
    <DropdownMenuSub v-if="node.branch">
      <DropdownMenuSubTrigger
        class="gap-1.5 py-1 text-xs"
        :style="{ paddingLeft: `${6 + depth * 12}px` }"
      >
        <span
          v-if="node.children.length"
          class="shrink-0 text-muted-foreground"
          @click.stop="emit('toggleFolder', folderKey(node))"
        >
          <ChevronRight
            class="h-3 w-3 transition-transform"
            :class="collapsed.has(folderKey(node)) ? '' : 'rotate-90'"
          />
        </span>
        <span v-else class="w-3 shrink-0" />
        <Folder v-if="node.children.length" class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <GitBranch v-else class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span
          class="truncate"
          :class="!remote && node.branch === currentBranch ? 'font-semibold' : ''"
        >
          {{ node.name }}
        </span>
        <!-- 尾组不加分隔边距:SubTrigger 内置箭头的 ml-auto 会把整组推到行最右,绿点紧贴箭头 -->
        <span class="flex shrink-0 items-center gap-1.5">
          <GitBranchTrackBadges
            v-if="!remote"
            :ahead="trackOf(node.branch)?.ahead ?? 0"
            :behind="trackOf(node.branch)?.behind ?? 0"
          />
          <span
            v-if="!remote && node.branch === currentBranch"
            class="h-1.5 w-1.5 shrink-0 rounded-full bg-green-500"
          />
        </span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent class="w-44">
        <DropdownMenuItem
          class="gap-2 text-xs"
          :disabled="locked || !!branchOp || (!remote && node.branch === currentBranch)"
          @click="emit('checkout', node.branch!, remote)"
        >
          <LogIn class="h-3.5 w-3.5" />
          {{ t("git.branch.checkout") }}
        </DropdownMenuItem>
        <template v-if="!remote">
          <DropdownMenuItem
            class="gap-2 text-xs"
            :disabled="locked || !!branchOp"
            @click="emit('pull', node.branch!)"
          >
            <Loader2 v-if="opOf(node.branch, 'pull')" class="h-3.5 w-3.5 animate-spin" />
            <ArrowDownToLine v-else class="h-3.5 w-3.5" />
            {{ t("git.branch.update") }}
          </DropdownMenuItem>
          <DropdownMenuItem
            class="gap-2 text-xs"
            :disabled="locked || !!branchOp"
            @click="emit('push', node.branch!)"
          >
            <Loader2 v-if="opOf(node.branch, 'push')" class="h-3.5 w-3.5 animate-spin" />
            <ArrowUpToLine v-else class="h-3.5 w-3.5" />
            {{ t("git.actions.push") }}
          </DropdownMenuItem>
        </template>
        <!-- 当前检出分支不可删除(git 会拒绝),直接禁用 -->
        <DropdownMenuItem
          class="gap-2 text-xs"
          variant="destructive"
          :disabled="locked || !!branchOp || (!remote && node.branch === currentBranch)"
          @click="emit('remove', node.branch!, remote)"
        >
          <Trash2 class="h-3.5 w-3.5" />
          {{ remote ? t("git.branch.deleteRemote") : t("git.branch.delete") }}
        </DropdownMenuItem>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
    <!-- 纯目录节点:自定义行,点击内联展开/收起 -->
    <div
      v-else
      class="flex cursor-pointer items-center gap-1.5 rounded-md py-1 pr-1.5 text-xs select-none hover:bg-accent"
      :style="{ paddingLeft: `${6 + depth * 12}px` }"
      @click="emit('toggleFolder', folderKey(node))"
    >
      <ChevronRight
        class="h-3 w-3 shrink-0 text-muted-foreground transition-transform"
        :class="collapsed.has(folderKey(node)) ? '' : 'rotate-90'"
      />
      <Globe v-if="remote && depth === 0" class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <Folder v-else class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <span class="truncate">{{ node.name }}</span>
    </div>
    <!-- 展开的子级:内联缩进渲染 -->
    <GitBranchTreeItems
      v-if="node.children.length && !collapsed.has(folderKey(node))"
      :nodes="node.children"
      :remote="remote"
      :depth="depth + 1"
      :current-branch="currentBranch"
      :track-by-name="trackByName"
      :branch-op="branchOp"
      :locked="locked"
      :collapsed="collapsed"
      @toggle-folder="(k) => emit('toggleFolder', k)"
      @checkout="(b, r) => emit('checkout', b, r)"
      @pull="(b) => emit('pull', b)"
      @push="(b) => emit('push', b)"
      @remove="(b, r) => emit('remove', b, r)"
    />
  </template>
</template>
