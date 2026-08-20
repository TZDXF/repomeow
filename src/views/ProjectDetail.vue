<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
import {
  ArrowLeft,
  BookOpen,
  FileText,
  FolderSync,
  FolderTree,
  Pencil,
  Star,
  TriangleAlert,
  Waypoints,
} from "@lucide/vue";
import { useLocalStorage } from "@vueuse/core";
import { Button } from "@/components/ui/button";
import GitStatusBar from "@/components/git/GitStatusBar.vue";
import GitRemoteLink from "@/components/git/GitRemoteLink.vue";
import WorktreePanel from "@/components/git/WorktreePanel.vue";
import WorktreeSwitcher from "@/components/git/WorktreeSwitcher.vue";
import OpenWithMenu from "@/components/open/OpenWithMenu.vue";
import DockerCompose from "@/components/project/DockerCompose.vue";
import SpringBootCard from "@/components/java/SpringBootCard.vue";
import ReadmeDrawer from "@/components/project/ReadmeDrawer.vue";
import RelocateProjectDialog from "@/components/project/RelocateProjectDialog.vue";
import DailyReportDialog from "@/components/report/DailyReportDialog.vue";
import CustomCommands from "@/components/scripts/CustomCommands.vue";
import PackageScripts from "@/components/scripts/PackageScripts.vue";
import TagPicker from "@/components/tags/TagPicker.vue";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import type { Project } from "@/types";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const store = useProjectsStore();

const project = computed(() => {
  const id = Number(route.params.id);
  return Number.isFinite(id) ? store.projects.find((p) => p.id === id) : undefined;
});

// 选中项目进入详情页时刷新本地工作区状态(走后端 15s 缓存,大仓库不重复跑 git status;
// 远端 fetch 由后端刷新循环调度,带退避治理,前端不再逐项目触发)
// 目录已失效的项目跳过,git 命令必然失败且没有展示意义
watch(
  () => project.value?.id,
  () => {
    if (project.value?.path_exists) {
      store.refreshGitStatus(project.value);
    }
  },
  { immediate: true },
);

// --- 重新指定目录弹窗 ---
const relocateOpen = ref(false);

// 切换项目时退出编辑态
watch(
  () => project.value?.id,
  () => {
    editingName.value = false;
    editingDesc.value = false;
    readmeOpen.value = false;
  },
);

// --- README 侧边栏 ---
const readmeOpen = ref(false);
// 项目根目录无 README 时不展示入口按钮;探测以 path 为凭据,避免切换项目时旧请求回写
const hasReadme = ref(false);
watch(
  () => (project.value?.path_exists ? project.value.path : null),
  async (path) => {
    if (!path) {
      hasReadme.value = false;
      return;
    }
    try {
      const has = await cmd<boolean>("has_readme", { path });
      if (project.value?.path === path) hasReadme.value = has;
    } catch {
      if (project.value?.path === path) hasReadme.value = false;
    }
  },
  { immediate: true },
);

// --- AI 日报弹窗 ---
const reportOpen = ref(false);

// --- worktree 工作区切换 ---
// activeWorktreePath: 详情页当前工作目录(null = 主工作区),按项目记忆在 localStorage;
// worktreeProject: 切到 worktree 时基于 project 的副本(覆盖 path/git),传给 git 状态/操作
// 与 npm/docker/自定义命令卡片——store 的 git 读写均以 project.path 执行并把结果回填 project.git,
// 用副本即可让操作落在 worktree 上,且不污染列表里主工作区的状态;自定义命令按 project_id
// 存取,副本保留原 id,增删改与「常用命令」标记仍跨工作区共享,仅执行目录跟随 worktree。
const worktreeOpen = ref(false);
const switcherRef = ref<InstanceType<typeof WorktreeSwitcher> | null>(null);
const activeWorktreePath = ref<string | null>(null);
const worktreeProject = ref<Project | null>(null);
/** 各项目记忆的工作区选择:projectId -> worktree 绝对路径(与 useCollapsibleOpen 同走 localStorage) */
const worktreeSelection = useLocalStorage<Record<string, string>>(
  "repomeow.worktree-selection",
  {},
);

// 进入/切换项目时恢复记忆的工作区;选中的 worktree 是否仍存在由 WorktreeSwitcher 加载
// 列表后校验,已删除时会把 activeWorktreePath 重置为 null(自动回退主工作区)
watch(
  () => project.value?.id,
  (id) => {
    activeWorktreePath.value = id != null ? (worktreeSelection.value[String(id)] ?? null) : null;
  },
  { immediate: true },
);

// 工作区变化:持久化选择;切到 worktree 时重建副本并拉取其 git 状态;切回主工作区
// (不含初始进入,初始刷新由上方 project.id watch 负责)时顺带刷新一次主工作区状态
watch(
  activeWorktreePath,
  (path, oldPath) => {
    const id = project.value?.id;
    if (id != null) {
      const next = { ...worktreeSelection.value };
      if (path) next[String(id)] = path;
      else delete next[String(id)];
      worktreeSelection.value = next;
    }
    if (path && project.value) {
      worktreeProject.value = { ...project.value, path, git: null };
      store.refreshGitStatus(worktreeProject.value);
    } else {
      worktreeProject.value = null;
      if (oldPath !== undefined && project.value?.path_exists) {
        store.refreshGitStatus(project.value);
      }
    }
  },
  { immediate: true },
);

// 项目基础信息变更(改名/改标签等会替换 store 中的对象)时同步副本的基础字段,
// 保留当前 worktree 路径与已拉取的 git 状态
watch(project, (p) => {
  if (p && worktreeProject.value) {
    worktreeProject.value = {
      ...p,
      path: worktreeProject.value.path,
      git: worktreeProject.value.git,
    };
  }
});

/** worktree 面板变更(增删/合回/变基)后:刷新切换下拉列表,并强制重拉
 * 当前工作区状态(变基/被合入会改变 ahead/behind 与未提交统计) */
function onWorktreeChanged() {
  switcherRef.value?.reload();
  const target = worktreeProject.value ?? project.value;
  if (target) store.refreshGitStatus(target, { force: true });
}

// --- 收藏切换(收藏项目在列表中置顶) ---
async function toggleFavorite() {
  if (!project.value) return;
  const favorite = !project.value.favorited_at;
  try {
    await store.setFavorite(project.value.id, favorite);
    toast.success(
      t(favorite ? "projects.actions.favoriteSuccess" : "projects.actions.unfavoriteSuccess", {
        name: project.value.name,
      }),
    );
  } catch (e) {
    toast.error(String(e));
  }
}

// --- 名称内联编辑 ---
const editingName = ref(false);
const draftName = ref("");
const nameInput = ref<HTMLInputElement | null>(null);

function startEditName() {
  if (!project.value) return;
  draftName.value = project.value.name;
  editingName.value = true;
  nextTick(() => nameInput.value?.select());
}

async function saveName() {
  if (!editingName.value || !project.value) return;
  editingName.value = false;
  const name = draftName.value.trim();
  if (!name || name === project.value.name) return;
  try {
    await store.updateProject(project.value.id, name, project.value.description);
    toast.success(t("projects.detail.saved"));
  } catch (e) {
    toast.error(String(e));
  }
}

// --- 描述内联编辑 ---
const editingDesc = ref(false);
const draftDesc = ref("");
const descInput = ref<HTMLTextAreaElement | null>(null);

function startEditDesc() {
  if (!project.value) return;
  draftDesc.value = project.value.description;
  editingDesc.value = true;
  nextTick(() => descInput.value?.focus());
}

async function saveDesc() {
  if (!editingDesc.value || !project.value) return;
  editingDesc.value = false;
  const description = draftDesc.value.trim();
  if (description === project.value.description) return;
  try {
    await store.updateProject(project.value.id, project.value.name, description);
    toast.success(t("projects.detail.saved"));
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <div v-if="project" class="flex h-full flex-col overflow-y-auto">
    <header class="shrink-0 border-b px-6 py-4">
      <div class="flex items-start justify-between gap-4">
        <div class="flex min-w-0 items-center gap-2">
          <Button
            variant="ghost"
            size="icon"
            class="h-8 w-8 shrink-0"
            :title="t('projects.detail.backToList')"
            @click="router.push('/')"
          >
            <ArrowLeft class="h-4 w-4" />
          </Button>
          <input
            v-if="editingName"
            ref="nameInput"
            v-model="draftName"
            class="h-8 w-72 max-w-full rounded-md border border-input bg-transparent px-2 text-lg font-semibold outline-none focus-visible:ring-1 focus-visible:ring-ring"
            @keydown.enter.prevent="saveName"
            @keydown.esc="editingName = false"
            @blur="saveName"
          />
          <h1
            v-else
            class="group flex min-w-0 cursor-pointer items-center gap-1.5 text-lg font-semibold"
            :title="t('projects.detail.editName')"
            @click="startEditName"
          >
            <span class="truncate">{{ project.name }}</span>
            <Pencil
              class="h-3.5 w-3.5 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100"
            />
          </h1>
          <!-- 打开远程仓库与工作区切换:跟随项目名展示(remote 为仓库级配置,与当前工作区无关) -->
          <template v-if="project.path_exists && project.git?.is_repo">
            <GitRemoteLink :project="project" />
            <WorktreeSwitcher
              ref="switcherRef"
              v-model:path="activeWorktreePath"
              :project="project"
              @manage="worktreeOpen = true"
            />
          </template>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <Button
            variant="outline"
            size="icon"
            class="h-9 w-9"
            :title="
              t(project.favorited_at ? 'projects.actions.unfavorite' : 'projects.actions.favorite')
            "
            @click="toggleFavorite"
          >
            <Star
              class="h-4 w-4"
              :class="project.favorited_at ? 'fill-yellow-400 text-yellow-400' : ''"
            />
          </Button>
          <Button variant="outline" size="sm" :title="t('report.title')" @click="reportOpen = true">
            <FileText class="h-4 w-4" />
            {{ t("ai.entry") }}
          </Button>
          <Button
            v-if="project.path_exists && hasReadme"
            variant="outline"
            size="sm"
            @click="readmeOpen = true"
          >
            <BookOpen class="h-4 w-4" />
            {{ t("readme.title") }}
          </Button>
          <Button
            v-if="project.path_exists"
            variant="outline"
            size="sm"
            :title="t('files.entry')"
            @click="router.push(`/projects/${project.id}/files`)"
          >
            <FolderTree class="h-4 w-4" />
            {{ t("files.entry") }}
          </Button>
          <OpenWithMenu v-if="project.path_exists" :project="worktreeProject ?? project" />
        </div>
      </div>

      <p
        class="mt-1 truncate pl-10 text-sm text-muted-foreground"
        :title="(worktreeProject ?? project).path"
      >
        {{ (worktreeProject ?? project).path }}
      </p>

      <div
        v-if="!project.path_exists"
        class="ml-10 mt-2 flex items-center gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
      >
        <TriangleAlert class="h-4 w-4 shrink-0" />
        <span class="min-w-0 flex-1">{{ t("projects.status.pathMissingHint") }}</span>
        <Button variant="outline" size="sm" class="shrink-0" @click="relocateOpen = true">
          <FolderSync class="h-4 w-4" />
          {{ t("projects.actions.relocate") }}
        </Button>
      </div>

      <div class="mt-1 pl-10">
        <textarea
          v-if="editingDesc"
          ref="descInput"
          v-model="draftDesc"
          rows="2"
          :placeholder="t('projects.detail.descPlaceholder')"
          class="w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
          @keydown.esc="editingDesc = false"
          @keydown.enter.ctrl.prevent="saveDesc"
          @blur="saveDesc"
        />
        <p
          v-else
          class="group flex w-fit cursor-pointer items-center gap-1.5 text-sm"
          :class="project.description ? '' : 'text-muted-foreground'"
          :title="t('projects.detail.editDesc')"
          @click="startEditDesc"
        >
          {{ project.description || t("projects.detail.addDesc") }}
          <Pencil
            class="h-3 w-3 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100"
          />
        </p>
      </div>

      <div class="mt-2.5 flex flex-wrap items-center gap-x-6 gap-y-2 pl-10">
        <TagPicker :project="project" />
        <template v-if="project.path_exists">
          <GitStatusBar :project="worktreeProject ?? project" />
          <Button
            v-if="project.git?.is_repo"
            variant="outline"
            size="xs"
            @click="router.push(`/projects/${project.id}/graph`)"
          >
            <Waypoints class="h-3.5 w-3.5" />
            {{ t("git.graph.title") }}
          </Button>
        </template>
      </div>
    </header>

    <div
      v-if="project.path_exists"
      class="grid items-start gap-4 p-6 [grid-template-columns:repeat(auto-fill,minmax(360px,1fr))]"
    >
      <PackageScripts :project="worktreeProject ?? project" />
      <DockerCompose :project="worktreeProject ?? project" />
      <SpringBootCard :project="worktreeProject ?? project" />
      <CustomCommands :project="worktreeProject ?? project" />
    </div>

    <ReadmeDrawer v-model:open="readmeOpen" :project="project" />
    <DailyReportDialog v-model:open="reportOpen" :preset-project-id="project.id" />
    <RelocateProjectDialog v-model:open="relocateOpen" :project="project" />
    <WorktreePanel
      v-model:open="worktreeOpen"
      :project="project"
      :active-path="activeWorktreePath"
      @changed="onWorktreeChanged"
    />
  </div>

  <div
    v-else
    class="flex h-full flex-col items-center justify-center gap-3 text-sm text-muted-foreground"
  >
    <p>{{ t("projects.detail.notFound") }}</p>
    <Button variant="outline" size="sm" @click="router.push('/')">{{
      t("projects.detail.backToListShort")
    }}</Button>
  </div>
</template>
