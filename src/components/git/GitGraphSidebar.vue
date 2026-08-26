<script setup lang="ts">
import { computed, ref, type Ref } from "vue";
import { useI18n } from "vue-i18n";
import {
  ArrowDownToLine,
  ArrowUpToLine,
  ChevronRight,
  Folder,
  GitBranch,
  Globe,
  Loader2,
  PanelLeftClose,
  PanelLeftOpen,
  Tag as TagIcon,
  Trash2,
} from "@lucide/vue";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { buildBranchTree, type BranchTreeNode } from "@/lib/branch-tree";
import type { GitBranches, GitBranchTrack } from "@/types";
import GitBranchTrackBadges from "./GitBranchTrackBadges.vue";

interface BranchTreeRow {
  node: BranchTreeNode;
  depth: number;
}

const props = defineProps<{
  branches: GitBranches;
  tags: string[];
  currentBranch: string;
  selectedBranch: string;
  branchOp: { branch: string; op: "pull" | "push" } | null;
}>();

const open = defineModel<boolean>("open", { required: true });

const emit = defineEmits<{
  selectBranch: [name: string];
  locateTag: [tag: string];
  pullBranch: [name: string];
  pushBranch: [name: string];
  deleteBranch: [name: string];
}>();

const { t } = useI18n();
const collapsedSections = ref<Set<string>>(new Set());
const collapsedFolders = ref<Set<string>>(new Set());

function toggleInSet(target: Ref<Set<string>>, key: string) {
  const next = new Set(target.value);
  if (next.has(key)) {
    next.delete(key);
  } else {
    next.add(key);
  }
  target.value = next;
}

function toggleSection(key: string) {
  toggleInSet(collapsedSections, key);
}

function toggleFolder(key: string) {
  toggleInSet(collapsedFolders, key);
}

function branchRows(names: string[], prefix: string): BranchTreeRow[] {
  const out: BranchTreeRow[] = [];
  const walk = (nodes: BranchTreeNode[], depth: number) => {
    for (const node of nodes) {
      out.push({ node, depth });
      if (node.children.length && !collapsedFolders.value.has(`${prefix}:${node.fullPath}`)) {
        walk(node.children, depth + 1);
      }
    }
  };
  walk(buildBranchTree(names), 0);
  return out;
}

const localRows = computed(() => branchRows(props.branches.local, "local"));
const remoteRows = computed(() => branchRows(props.branches.remote, "remote"));
const trackByName = computed(() => {
  const result = new Map<string, GitBranchTrack>();
  for (const track of props.branches.tracking) {
    result.set(track.name, track);
  }
  return result;
});

function trackOf(branch: string | null) {
  return branch ? trackByName.value.get(branch) : undefined;
}

function onBranchRowClick(prefix: string, row: BranchTreeRow) {
  if (row.node.branch) {
    emit("selectBranch", row.node.branch);
  } else if (row.node.children.length) {
    toggleFolder(`${prefix}:${row.node.fullPath}`);
  }
}
</script>

<template>
  <aside v-if="open" class="flex w-56 shrink-0 flex-col border-r">
    <div class="flex items-center justify-end px-2 pt-1.5">
      <button
        class="rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="t('git.graph.toggleSidebar')"
        @click="open = false"
      >
        <PanelLeftClose class="h-3.5 w-3.5" />
      </button>
    </div>
    <div class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pt-1 pb-2">
      <template v-if="branches.local.length">
        <button
          class="flex items-center gap-1 px-1 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase transition-colors hover:text-foreground"
          @click="toggleSection('local')"
        >
          <ChevronRight
            class="h-3 w-3 transition-transform"
            :class="collapsedSections.has('local') ? '' : 'rotate-90'"
          />
          {{ t("git.branch.local") }}
        </button>
        <template v-if="!collapsedSections.has('local')">
          <ContextMenu v-for="row in localRows" :key="`local:${row.node.fullPath}`">
            <ContextMenuTrigger as-child :disabled="!row.node.branch">
              <button
                class="flex w-full items-center gap-1.5 rounded-sm py-1 pr-2 text-left text-xs transition-colors hover:bg-accent"
                :class="[
                  row.node.branch === currentBranch ? 'font-semibold' : '',
                  row.node.branch && selectedBranch === row.node.branch ? 'bg-accent' : '',
                ]"
                :style="{ paddingLeft: `${8 + row.depth * 12}px` }"
                @click="onBranchRowClick('local', row)"
              >
                <span
                  v-if="row.node.children.length"
                  class="shrink-0 text-muted-foreground"
                  @click.stop="toggleFolder(`local:${row.node.fullPath}`)"
                >
                  <ChevronRight
                    class="h-3 w-3 transition-transform"
                    :class="collapsedFolders.has(`local:${row.node.fullPath}`) ? '' : 'rotate-90'"
                  />
                </span>
                <span v-else class="w-3 shrink-0" />
                <Folder
                  v-if="row.node.children.length"
                  class="h-3 w-3 shrink-0 text-muted-foreground"
                />
                <GitBranch v-else class="h-3 w-3 shrink-0 text-muted-foreground" />
                <span class="truncate">{{ row.node.name }}</span>
                <span class="ml-auto flex shrink-0 items-center gap-1.5">
                  <Loader2
                    v-if="branchOp?.branch === row.node.branch"
                    class="h-3 w-3 animate-spin text-muted-foreground"
                  />
                  <GitBranchTrackBadges
                    :ahead="trackOf(row.node.branch)?.ahead ?? 0"
                    :behind="trackOf(row.node.branch)?.behind ?? 0"
                  />
                  <span
                    v-if="row.node.branch === currentBranch"
                    class="h-1.5 w-1.5 shrink-0 rounded-full bg-green-500"
                  />
                </span>
              </button>
            </ContextMenuTrigger>
            <ContextMenuContent v-if="row.node.branch" class="w-40">
              <ContextMenuItem
                class="gap-2 text-xs"
                :disabled="!!branchOp"
                @click="emit('pullBranch', row.node.branch!)"
              >
                <ArrowDownToLine class="h-3.5 w-3.5" />
                {{ t("git.actions.pull") }}
              </ContextMenuItem>
              <ContextMenuItem
                class="gap-2 text-xs"
                :disabled="!!branchOp"
                @click="emit('pushBranch', row.node.branch!)"
              >
                <ArrowUpToLine class="h-3.5 w-3.5" />
                {{ t("git.actions.push") }}
              </ContextMenuItem>
              <ContextMenuItem
                class="gap-2 text-xs"
                variant="destructive"
                :disabled="!!branchOp || row.node.branch === currentBranch"
                @click="emit('deleteBranch', row.node.branch!)"
              >
                <Trash2 class="h-3.5 w-3.5" />
                {{ t("git.branch.delete") }}
              </ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        </template>
      </template>

      <template v-if="branches.remote.length">
        <button
          class="flex items-center gap-1 px-1 pt-2 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase transition-colors hover:text-foreground"
          @click="toggleSection('remote')"
        >
          <ChevronRight
            class="h-3 w-3 transition-transform"
            :class="collapsedSections.has('remote') ? '' : 'rotate-90'"
          />
          {{ t("git.branch.remote") }}
        </button>
        <template v-if="!collapsedSections.has('remote')">
          <button
            v-for="row in remoteRows"
            :key="`remote:${row.node.fullPath}`"
            class="flex items-center gap-1.5 rounded-sm py-1 pr-2 text-left text-xs transition-colors hover:bg-accent"
            :class="row.node.branch && selectedBranch === row.node.branch ? 'bg-accent' : ''"
            :style="{ paddingLeft: `${8 + row.depth * 12}px` }"
            @click="onBranchRowClick('remote', row)"
          >
            <span
              v-if="row.node.children.length"
              class="shrink-0 text-muted-foreground"
              @click.stop="toggleFolder(`remote:${row.node.fullPath}`)"
            >
              <ChevronRight
                class="h-3 w-3 transition-transform"
                :class="collapsedFolders.has(`remote:${row.node.fullPath}`) ? '' : 'rotate-90'"
              />
            </span>
            <span v-else class="w-3 shrink-0" />
            <Globe
              v-if="row.node.children.length && row.depth === 0"
              class="h-3 w-3 shrink-0 text-muted-foreground"
            />
            <Folder
              v-else-if="row.node.children.length"
              class="h-3 w-3 shrink-0 text-muted-foreground"
            />
            <GitBranch v-else class="h-3 w-3 shrink-0 text-muted-foreground" />
            <span class="truncate">{{ row.node.name }}</span>
          </button>
        </template>
      </template>

      <template v-if="tags.length">
        <button
          class="flex items-center gap-1 px-1 pt-2 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase transition-colors hover:text-foreground"
          @click="toggleSection('tags')"
        >
          <ChevronRight
            class="h-3 w-3 transition-transform"
            :class="collapsedSections.has('tags') ? '' : 'rotate-90'"
          />
          {{ t("git.graph.tags") }}
        </button>
        <template v-if="!collapsedSections.has('tags')">
          <button
            v-for="tag in tags"
            :key="tag"
            class="flex items-center gap-1.5 rounded-sm px-2 py-1 text-left text-xs transition-colors hover:bg-accent"
            @click="emit('locateTag', tag)"
          >
            <TagIcon class="h-3 w-3 shrink-0 text-amber-600 dark:text-amber-400" />
            <span class="truncate">{{ tag }}</span>
          </button>
        </template>
      </template>
    </div>
  </aside>

  <button
    v-else
    class="flex w-8 shrink-0 items-start justify-center border-r pt-2.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
    :title="t('git.graph.toggleSidebar')"
    @click="open = true"
  >
    <PanelLeftOpen class="h-3.5 w-3.5" />
  </button>
</template>
