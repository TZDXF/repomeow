<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Check, ChevronDown, FolderGit2, GitBranch, Loader2, Settings2 } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useProjectsStore } from "@/stores/projects";
import { displayRelativeTo } from "@/lib/path";
import type { GitWorktree, Project } from "@/types";

/**
 * 工作区切换下拉:在主工作区与各 worktree 之间切换详情页的当前工作目录。
 * model 为选中的 worktree 绝对路径,null 表示主工作区;列表在挂载与每次打开
 * 下拉时刷新,当前选中的 worktree 已不存在(被管理面板或外部删除)时自动回退
 * 主工作区。管理操作(新建/删除/合回/变基)经 manage 事件交给 WorktreePanel。
 */
const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const path = defineModel<string | null>("path", { required: true });
const emit = defineEmits<{ manage: [] }>();

const store = useProjectsStore();

const worktrees = ref<GitWorktree[]>([]);
const loading = ref(false);
const open = ref(false);

const mainWorktree = computed(() => worktrees.value.find((w) => w.is_main));
/** 主工作区之外的 linked worktree */
const linkedWorktrees = computed(() => worktrees.value.filter((w) => !w.is_main));

/** 触发按钮显示名:仅选中 linked worktree 时展示其分支名/短 hash,主工作区只显示图标 */
const currentLabel = computed(() => {
  const w = worktrees.value.find((x) => x.path === path.value);
  // 列表尚未加载到时先用通用文案兜底(加载完成会校验并可能回退主工作区)
  if (!w) return t("git.worktree.workspace");
  return w.branch ?? w.head.slice(0, 7);
});

/** 相对主工作区根的路径展示,与 WorktreePanel 的 displayPath 一致 */
function displayPath(w: GitWorktree) {
  return displayRelativeTo(mainWorktree.value?.path ?? "", w.path);
}

async function load() {
  loading.value = true;
  try {
    worktrees.value = await store.listWorktrees(props.project);
    // 当前选中的 worktree 已不存在时回退主工作区(父组件会同步清除持久化记录)
    if (path.value && !worktrees.value.some((w) => w.path === path.value)) {
      path.value = null;
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

watch(() => props.project.id, load, { immediate: true });
// 每次打开下拉刷新列表,保证面板外(如命令行)的增删也能看到
watch(open, (v) => {
  if (v) load();
});

function select(target: string | null) {
  if (target !== path.value) path.value = target;
}

/** 管理面板创建/删除 worktree 后由父组件调用,刷新列表并校验当前选中 */
defineExpose({ reload: load });
</script>

<template>
  <!-- 主工作区且无其他 worktree:退化为普通按钮,点击直接打开管理面板(新建第一个 worktree) -->
  <Button
    v-if="!path && linkedWorktrees.length === 0"
    variant="outline"
    size="xs"
    :title="t('git.worktree.manage')"
    @click="emit('manage')"
  >
    <FolderGit2 class="h-3.5 w-3.5" />
    <Settings2 class="h-3 w-3 opacity-60" />
  </Button>
  <DropdownMenu v-else v-model:open="open">
    <DropdownMenuTrigger as-child>
      <Button variant="outline" size="xs" :title="t('git.worktree.switchWorkspace')">
        <FolderGit2 class="h-3.5 w-3.5" />
        <span v-if="path" class="max-w-48 truncate">{{ currentLabel }}</span>
        <ChevronDown class="h-3 w-3 opacity-60" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="start" class="w-64">
      <DropdownMenuLabel class="text-xs">{{ t("git.worktree.workspace") }}</DropdownMenuLabel>
      <DropdownMenuItem v-if="loading" disabled class="gap-2 text-xs">
        <Loader2 class="h-3.5 w-3.5 animate-spin" />
        {{ t("common.loading") }}
      </DropdownMenuItem>
      <template v-else>
        <!-- 主工作区 -->
        <DropdownMenuItem class="gap-2 text-xs" @click="select(null)">
          <Check v-if="!path" class="h-3.5 w-3.5 shrink-0 text-primary" />
          <span v-else class="h-3.5 w-3.5 shrink-0" />
          <GitBranch class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate">
            {{ mainWorktree?.branch ?? project.git?.branch ?? t("git.worktree.main") }}
          </span>
          <Badge variant="secondary" class="ml-auto shrink-0 text-[10px]">
            {{ t("git.worktree.main") }}
          </Badge>
        </DropdownMenuItem>
        <!-- 各 linked worktree -->
        <DropdownMenuItem
          v-for="w in linkedWorktrees"
          :key="w.path"
          class="gap-2 text-xs"
          @click="select(w.path)"
        >
          <Check v-if="path === w.path" class="h-3.5 w-3.5 shrink-0 text-primary" />
          <span v-else class="h-3.5 w-3.5 shrink-0" />
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-1.5">
              <span class="truncate">{{ w.branch ?? w.head.slice(0, 7) }}</span>
              <Badge v-if="w.detached" variant="outline" class="shrink-0 text-[10px]">
                {{ t("git.worktree.detached") }}
              </Badge>
            </div>
            <p class="truncate font-mono text-[11px] text-muted-foreground" :title="w.path">
              {{ displayPath(w) }}
            </p>
          </div>
        </DropdownMenuItem>
      </template>
      <DropdownMenuSeparator />
      <DropdownMenuItem class="gap-2 text-xs" @click="emit('manage')">
        <Settings2 class="h-3.5 w-3.5" />
        {{ t("git.worktree.manage") }}
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
