<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  Eraser,
  Eye,
  EyeOff,
  Leaf,
  ListChecks,
  MoreHorizontal,
  Package,
  Play,
  Star,
  TestTube2,
} from "@lucide/vue";
import type { AcceptableValue } from "reka-ui";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { JDK_FOLLOW_DEFAULT, JDK_SYSTEM_PATH, resolveJavaHome } from "@/lib/jdk";
import { cmd, runInTerminal } from "@/lib/tauri";
import { usePinsStore } from "@/stores/pins";
import { useProjectAssetsStore } from "@/stores/project-assets";
import { useProjectOverviewStore } from "@/stores/project-overview";
import { useSettingsStore } from "@/stores/settings";
import type { HiddenItem, HiddenKind, JavaBuildGroup, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const assetsStore = useProjectAssetsStore();
const overviewStore = useProjectOverviewStore();
const settingsStore = useSettingsStore();
const pinsStore = usePinsStore();

/** 扫描结果来自共享 store(与 npm/docker 卡片合并为一次后端扫描) */
const groups = computed(() => assetsStore.assetsOf(props.project)?.java_builds ?? []);
const loaded = ref(false);
/** 已隐藏的分组("<dir>\n<tool>")与临时显示开关(灰显,可逐条恢复) */
const hiddenGroups = ref<Set<string>>(new Set());
const showHidden = ref(false);

const groupKey = (g: JavaBuildGroup) => `${g.dir}\n${g.tool}`;

const hiddenCount = computed(() => hiddenGroups.value.size);

interface DisplayGroup {
  group: JavaBuildGroup;
  hidden: boolean;
}

/** 当前应展示的分组:默认过滤隐藏;showHidden 时全部显示但标记 hidden 灰显 */
const displayGroups = computed<DisplayGroup[]>(() =>
  groups.value
    .map((g) => ({ group: g, hidden: hiddenGroups.value.has(groupKey(g)) }))
    .filter((x) => showHidden.value || !x.hidden),
);

// path 一并作为 watch 源:详情页切换工作区(worktree)时传入的是 id 相同、
// path 不同的 project 副本,需要按新工作目录重新扫描;refresh 内部去重,
// 与 PackageScripts / DockerCompose 同时挂载只触发一次扫描
watch(
  () => [props.project.id, props.project.path],
  async () => {
    showHidden.value = false;
    pinsStore.ensureLoaded();
    // stale-while-revalidate:旧扫描结果与旧隐藏项都齐备时,立即按旧数据渲染
    // (隐藏项必须先就位,否则已隐藏分组会在首屏闪现再消失)
    const staleOverview = overviewStore.cached(props.project.id);
    if (staleOverview) applyHiddenItems(staleOverview.hidden_items);
    const hasStale = !!assetsStore.assetsOf(props.project) && !!staleOverview;
    loaded.value = hasStale;
    // refresh 内部去重:与 PackageScripts / DockerCompose 同时挂载只触发一次
    // scan_project_assets / get_project_overview;两者失败均回退旧数据/空数据,不抛错
    const [, overview] = await Promise.all([
      assetsStore.refresh(props.project),
      overviewStore.refresh(props.project.id),
    ]);
    applyHiddenItems(overview.hidden_items);
    loaded.value = true;
  },
  { immediate: true },
);

function applyHiddenItems(items: HiddenItem[]) {
  hiddenGroups.value = new Set(items.filter((i) => i.kind === "javaBuild").map((i) => i.targetKey));
}

const defaultJdk = computed(() =>
  settingsStore.jdkList.find((j) => j.id === settingsStore.defaultJdkId),
);

/** 当前项目的 JDK 选择值(Select 的 v-model) */
const jdkMode = computed(() => settingsStore.projectJdkMap[props.project.id] ?? JDK_FOLLOW_DEFAULT);

async function onJdkChange(value: AcceptableValue) {
  if (typeof value !== "string") return;
  // 跟随默认不落库(删除默认项后自动回退语义),其余显式持久化
  await settingsStore.setProjectJdk(props.project.id, value === JDK_FOLLOW_DEFAULT ? "" : value);
}

/** 运行时应注入的 JAVA_HOME(项目选择 > 默认 JDK;未配置走系统环境) */
const javaHome = computed(() => resolveJavaHome(settingsStore, props.project.id));

function groupLabel(g: JavaBuildGroup): string {
  return g.dir === "." ? t("java.rootDir") : g.dir;
}

/** 统一执行入口:多模块工程由后端指定执行目录(run_dir 统一为项目根) */
async function runCommand(g: JavaBuildGroup, command: string) {
  const cwd = g.run_dir === "." ? undefined : `${props.project.path}/${g.run_dir}`;
  try {
    await runInTerminal(props.project, command, cwd, javaHome.value);
    toast.success(t("java.started", { command }));
  } catch (e) {
    toast.error(String(e));
  }
}

async function setHidden(kind: HiddenKind, key: string, hidden: boolean) {
  try {
    await cmd("set_hidden_item", { projectId: props.project.id, kind, targetKey: key, hidden });
    // 同步 overview 缓存,下次进入详情时 stale 首屏就是最新隐藏状态
    overviewStore.setHiddenLocal(props.project.id, kind, key, hidden);
    const next = new Set(hiddenGroups.value);
    if (hidden) next.add(key);
    else next.delete(key);
    hiddenGroups.value = next;
  } catch (e) {
    toast.error(String(e));
  }
}

/** 切换「常用命令」标记(托盘弹窗中可快速执行,JDK 选择对托盘同样生效) */
async function togglePin(g: JavaBuildGroup) {
  const key = groupKey(g);
  const pinned = pinsStore.isPinned(props.project.id, "javaBuild", key);
  try {
    await pinsStore.setPinned(
      props.project.id,
      {
        kind: "javaBuild",
        targetKey: key,
        label: g.dir === "." ? "Spring Boot" : `Spring Boot (${g.dir})`,
        command: g.run_command,
        // 存相对目录而非绝对路径:项目迁移目录(Relocate)后标记仍可用,执行时再拼接 project.path
        cwd: g.run_dir === "." ? undefined : g.run_dir,
      },
      !pinned,
    );
  } catch (e) {
    toast.error(String(e));
  }
}

/** 更多操作菜单的图标与颜色(maven/gradle 通用) */
const ACTION_ICONS: Record<string, typeof Play> = {
  "java.clean": Eraser,
  "java.package": Package,
  "java.install": Package,
  "java.test": TestTube2,
  "java.build": ListChecks,
};
</script>

<template>
  <!-- 无 Spring Boot 构建文件时整体不渲染卡片,与 npm/docker 卡片一致;
       全部隐藏时保留头部,以便通过「显示已隐藏」恢复 -->
  <Card v-if="loaded && (displayGroups.length || hiddenCount)" class="group/card">
    <CardHeader class="pb-3">
      <!-- min-h-6 与 hover 才显示的 h-6 按钮同高,避免按钮出现时头部跳动 -->
      <CardTitle class="flex min-h-6 items-center gap-2 text-sm font-semibold">
        <Leaf class="h-4 w-4" />
        {{ t("java.title") }}
        <div class="ml-auto flex items-center gap-1.5">
          <span class="text-xs font-normal text-muted-foreground">{{ t("java.jdkLabel") }}</span>
          <Select :model-value="jdkMode" @update:model-value="onJdkChange">
            <SelectTrigger class="h-6 w-36 shrink-0 overflow-hidden text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem :value="JDK_FOLLOW_DEFAULT">
                  {{
                    defaultJdk
                      ? t("java.followDefault", { name: defaultJdk.name })
                      : t("java.followDefaultNone")
                  }}
                </SelectItem>
                <SelectItem :value="JDK_SYSTEM_PATH">{{ t("java.systemPath") }}</SelectItem>
              </SelectGroup>
              <SelectGroup v-if="settingsStore.jdkList.length">
                <SelectLabel>{{ t("java.jdkGroupLabel") }}</SelectLabel>
                <SelectItem v-for="jdk in settingsStore.jdkList" :key="jdk.id" :value="jdk.id">
                  {{ jdk.name }}
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>
        <template v-if="hiddenCount">
          <Button
            variant="ghost"
            size="icon"
            class="h-6 w-6 shrink-0 text-muted-foreground"
            :class="{ 'hidden group-hover/card:inline-flex': !showHidden }"
            :title="showHidden ? t('common.hideShown') : t('common.showHidden')"
            @click="showHidden = !showHidden"
          >
            <EyeOff v-if="showHidden" class="h-3.5 w-3.5" />
            <Eye v-else class="h-3.5 w-3.5" />
          </Button>
        </template>
      </CardTitle>
    </CardHeader>
    <CardContent>
      <div class="flex flex-col gap-1">
        <div
          v-for="d in displayGroups"
          :key="`${project.id}:${d.group.dir}:${d.group.tool}`"
          class="group flex min-h-8 items-center gap-2 rounded-md px-2 py-1 hover:bg-accent"
          :class="{ 'opacity-50': d.hidden }"
        >
          <Button
            variant="ghost"
            size="icon"
            class="h-6 w-6 shrink-0 text-emerald-600"
            :title="t('java.run')"
            @click="runCommand(d.group, d.group.run_command)"
          >
            <Play class="h-3.5 w-3.5" />
          </Button>
          <span
            class="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium uppercase"
            :class="
              d.group.tool === 'maven'
                ? 'bg-orange-500/10 text-orange-600 dark:text-orange-400'
                : 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
            "
          >
            {{ d.group.tool }}
          </span>
          <span
            class="min-w-0 shrink-0 truncate font-mono text-xs font-medium"
            :title="d.group.dir === '.' ? undefined : d.group.dir"
          >
            {{ groupLabel(d.group) }}
          </span>
          <span
            class="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground"
            :title="d.group.run_command"
          >
            {{ d.group.run_command }}
          </span>
          <Button
            variant="ghost"
            size="icon"
            class="h-6 w-6 shrink-0"
            :class="
              pinsStore.isPinned(project.id, 'javaBuild', `${d.group.dir}\n${d.group.tool}`)
                ? 'text-yellow-500'
                : 'hidden text-muted-foreground group-hover:inline-flex'
            "
            :title="
              pinsStore.isPinned(project.id, 'javaBuild', `${d.group.dir}\n${d.group.tool}`)
                ? t('pins.unmark')
                : t('pins.mark')
            "
            @click="togglePin(d.group)"
          >
            <Star
              class="h-3.5 w-3.5"
              :class="{
                'fill-yellow-400': pinsStore.isPinned(
                  project.id,
                  'javaBuild',
                  `${d.group.dir}\n${d.group.tool}`,
                ),
              }"
            />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-6 w-6 shrink-0"
            :class="
              d.hidden
                ? 'text-muted-foreground'
                : 'hidden text-muted-foreground group-hover:inline-flex'
            "
            :title="d.hidden ? t('common.unhide') : t('common.hide')"
            @click="setHidden('javaBuild', `${d.group.dir}\n${d.group.tool}`, !d.hidden)"
          >
            <Eye v-if="d.hidden" class="h-3.5 w-3.5" />
            <EyeOff v-else class="h-3.5 w-3.5" />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger as-child>
              <Button
                variant="ghost"
                size="icon"
                class="h-6 w-6 shrink-0 text-muted-foreground"
                :title="t('java.more')"
              >
                <MoreHorizontal class="h-3.5 w-3.5" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" class="w-36">
              <DropdownMenuItem
                v-for="action in d.group.more_actions"
                :key="action.key"
                class="gap-2 text-xs"
                :title="action.command"
                @click="runCommand(d.group, action.command)"
              >
                <component :is="ACTION_ICONS[action.key] ?? ListChecks" class="h-3.5 w-3.5" />
                {{ t(action.key) }}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <p v-if="!settingsStore.jdkList.length" class="px-2 text-xs text-muted-foreground">
          {{ t("java.noJdkHint") }}
          <RouterLink to="/settings" class="underline underline-offset-2 hover:text-foreground">
            {{ t("java.goSettings") }}
          </RouterLink>
        </p>
      </div>
    </CardContent>
  </Card>
</template>
