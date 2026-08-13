<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ChevronRight,
  Container,
  Download,
  Eye,
  EyeOff,
  FileCode,
  Hammer,
  ImageDown,
  MoreHorizontal,
  Play,
  RotateCw,
  Square,
  Star,
} from "@lucide/vue";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useCollapsibleOpen } from "@/composables/useCollapsibleOpen";
import { cmd, runInTerminal } from "@/lib/tauri";
import { usePinsStore } from "@/stores/pins";
import { useHiddenItemsStore } from "@/stores/hidden-items";
import { useProjectAssetsStore } from "@/stores/project-assets";
import type { ComposeFile, ComposeServiceState, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const pinsStore = usePinsStore();
const assetsStore = useProjectAssetsStore();
const hiddenStore = useHiddenItemsStore();

const { isOpen, setOpen } = useCollapsibleOpen("compose");

/** 扫描结果来自共享 store(与 PackageScripts 卡片合并为一次后端扫描) */
const files = computed(() => assetsStore.assetsOf(props.project.id)?.compose_files ?? []);
const loaded = ref(false);
/** 各 compose 文件展开状态,key 为文件路径 */
const openStates = ref<Record<string, boolean>>({});
/** 服务运行状态,key 为 `${file.path}\n${service}`;无记录表示未创建/docker 不可用 */
const statuses = ref<Record<string, ComposeServiceState>>({});
const refreshing = ref(false);
/** 已隐藏的 compose 文件路径 */
const hiddenFiles = ref<Set<string>>(new Set());
/** 临时显示已隐藏文件(灰显,可逐个恢复) */
const showHidden = ref(false);

const stateKey = (f: ComposeFile, name: string) => `${f.path}\n${name}`;

/** 服务行「更多」菜单打开中的行 key:关闭时延迟清除,保证关闭动画期间触发按钮仍显示,内容锚点不丢失 */
const openMoreKey = ref<string | null>(null);

function onMoreOpenChange(key: string, open: boolean) {
  if (open) {
    openMoreKey.value = key;
  } else {
    // 菜单关闭动画 100ms,动画结束后再允许按钮随 group-hover 隐藏
    setTimeout(() => {
      if (openMoreKey.value === key) openMoreKey.value = null;
    }, 150);
  }
}

const hiddenCount = computed(() => hiddenFiles.value.size);

/** 当前应展示的文件:过滤隐藏文件;showHidden 时全部显示但标记 hidden 灰显 */
const displayFiles = computed(() =>
  files.value
    .map((f) => ({ file: f, hidden: hiddenFiles.value.has(f.path) }))
    .filter((x) => showHidden.value || !x.hidden),
);

// path 一并作为 watch 源:详情页切换工作区(worktree)时传入的是 id 相同、
// path 不同的 project 副本,需要按新工作目录重新扫描
watch(
  () => [props.project.id, props.project.path],
  async () => {
    loaded.value = false;
    showHidden.value = false;
    try {
      // refresh 内部去重:与 PackageScripts 同时挂载只触发一次 scan_project_assets / list_hidden_items
      const [, items] = await Promise.all([
        assetsStore.refresh(props.project),
        hiddenStore.refresh(props.project.id),
      ]);
      hiddenFiles.value = new Set(
        items.filter((i) => i.kind === "composeFile").map((i) => i.targetKey),
      );
    } catch {
      hiddenFiles.value = new Set();
    } finally {
      loaded.value = true;
    }
    openStates.value = Object.fromEntries(
      files.value.map((f) => [
        f.path,
        isOpen(`${props.project.id}:${f.path}`, files.value.length === 1),
      ]),
    );
    pinsStore.ensureLoaded();
    loadStatuses();
  },
  { immediate: true },
);

function onToggle(f: ComposeFile, open: boolean) {
  openStates.value[f.path] = open;
  setOpen(`${props.project.id}:${f.path}`, open);
}

async function toggleFileHidden(path: string, hidden: boolean) {
  try {
    await cmd("set_hidden_item", {
      projectId: props.project.id,
      kind: "composeFile",
      targetKey: path,
      hidden: !hidden,
    });
    const next = new Set(hiddenFiles.value);
    if (hidden) next.delete(path);
    else next.add(path);
    hiddenFiles.value = next;
    // 恢复显示的文件需要补查服务状态;隐藏则无需操作(下次刷新自然跳过)
    if (hidden) loadStatuses();
  } catch (e) {
    toast.error(String(e));
  }
}

/** 批量查询各 compose 文件的服务运行状态(失败静默,全部按未知处理);已隐藏文件不查询 */
async function loadStatuses() {
  // 隐藏的文件不展示服务状态,跳过可减少大部分 docker 进程调用;showHidden 时才一并查询
  const targets = files.value.filter((f) => showHidden.value || !hiddenFiles.value.has(f.path));
  if (!targets.length) {
    statuses.value = {};
    return;
  }
  refreshing.value = true;
  try {
    // 单次 IPC 批量查询,后端并行执行各文件的 docker compose ps
    const results = await cmd<ComposeServiceState[][]>("compose_ps_batch", {
      path: props.project.path,
      files: targets.map((f) => f.path),
    });
    const map: Record<string, ComposeServiceState> = {};
    targets.forEach((f, i) => {
      for (const st of results[i] ?? []) map[stateKey(f, st.name)] = st;
    });
    statuses.value = map;
  } catch {
    statuses.value = {};
  } finally {
    refreshing.value = false;
  }
}

// 临时显示已隐藏文件时需要补查它们的服务状态
watch(showHidden, (v) => {
  if (v) loadStatuses();
});

function stateOf(f: ComposeFile, name: string): ComposeServiceState | undefined {
  return statuses.value[stateKey(f, name)];
}

/** 状态点颜色:绿=运行中;黄=容器存在但未运行(exited 等);灰=未创建或 docker 不可用 */
function dotClass(f: ComposeFile, name: string): string {
  const st = stateOf(f, name);
  if (!st) return "bg-muted-foreground/40";
  return st.running ? "bg-emerald-500" : "bg-amber-500";
}

function stateTitle(f: ComposeFile, name: string): string {
  const st = stateOf(f, name);
  if (!st) return t("docker.statusUnknown");
  return st.status || t(st.running ? "docker.running" : "docker.stopped");
}

/** 在浏览器访问服务暴露到宿主机的端口 */
async function openPort(port: number) {
  try {
    await openUrl(`http://localhost:${port}`);
  } catch (e) {
    toast.error(String(e));
  }
}

/** compose 标记存的 command 为基础前缀,执行动作在托盘弹窗中点击时拼接 */
function composeBaseCommand(file: ComposeFile): string {
  return `docker compose -f "${file.path}"`;
}

/** 切换 compose 文件的「常用命令」标记 */
async function toggleFilePin(file: ComposeFile) {
  const pinned = pinsStore.isPinned(props.project.id, "composeFile", file.path);
  try {
    await pinsStore.setPinned(
      props.project.id,
      {
        kind: "composeFile",
        targetKey: file.path,
        label: file.path,
        command: composeBaseCommand(file),
      },
      !pinned,
    );
  } catch (e) {
    toast.error(String(e));
  }
}

/** 切换单个服务的「常用命令」标记 */
async function toggleServicePin(file: ComposeFile, service: string) {
  const key = `${file.path}\n${service}`;
  const pinned = pinsStore.isPinned(props.project.id, "composeService", key);
  try {
    await pinsStore.setPinned(
      props.project.id,
      {
        kind: "composeService",
        targetKey: key,
        label: service,
        command: composeBaseCommand(file),
      },
      !pinned,
    );
  } catch (e) {
    toast.error(String(e));
  }
}

/** 在项目终端执行 docker compose 命令;service 为空表示作用于该文件的所有服务 */
async function run(
  file: ComposeFile,
  action: "up -d" | "up -d --build" | "build" | "restart" | "down" | "stop",
  service?: string,
) {
  const args = `-f "${file.path}" ${service ? `${action} ${service}` : action}`;
  try {
    await runInTerminal(props.project, `docker compose ${args}`);
    toast.success(t("docker.started", { name: service ?? file.file_name }));
    // 命令在新终端窗口中异步执行,延迟刷新一次状态(拉取镜像时可能仍偏早,可手动刷新)
    setTimeout(loadStatuses, 4000);
  } catch (e) {
    toast.error(String(e));
  }
}

/** 导出服务的容器文件系统 / 镜像为 tar 包(save 对话框选路径,后端直接执行) */
async function exportService(file: ComposeFile, service: string, kind: "container" | "image") {
  try {
    const dest = await save({
      title: t(kind === "container" ? "docker.exportContainer" : "docker.exportImage"),
      defaultPath: `${service}-${kind}.tar`,
      filters: [{ name: "Tar", extensions: ["tar"] }],
    });
    if (!dest) return;
    await cmd("compose_export", {
      path: props.project.path,
      file: file.path,
      service,
      kind,
      dest,
    });
    toast.success(t("docker.exported", { path: dest }));
  } catch (e) {
    toast.error(String(e));
  }
}

/** 导出 compose 文件全部服务:选一个目录,逐服务生成 `<service>-<kind>.tar` */
async function exportAll(file: ComposeFile, kind: "container" | "image") {
  try {
    const dest = await open({
      directory: true,
      title: t(kind === "container" ? "docker.exportContainer" : "docker.exportImage"),
    });
    if (!dest) return;
    await cmd("compose_export", {
      path: props.project.path,
      file: file.path,
      service: "",
      kind,
      dest,
    });
    toast.success(t("docker.exported", { path: dest }));
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <!-- 全部隐藏时保留头部,以便通过「显示已隐藏」恢复 -->
  <Card v-if="loaded && (displayFiles.length || hiddenCount)" class="group/card">
    <CardHeader class="pb-3">
      <!-- min-h-6 与 hover 才显示的 h-6 按钮同高,避免按钮出现时头部跳动 -->
      <CardTitle class="flex min-h-6 items-center gap-2 text-sm font-semibold">
        <Container class="h-4 w-4" />
        {{ t("docker.title") }}
        <template v-if="hiddenCount">
          <Button
            variant="ghost"
            size="icon"
            class="ml-auto h-6 w-6 shrink-0 text-muted-foreground"
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
      <ScrollArea class="max-h-[320px]">
        <div class="flex flex-col">
          <Collapsible
            v-for="(d, i) in displayFiles"
            :key="`${project.id}:${d.file.path}`"
            v-slot="{ open }"
            :open="files.length > 1 ? openStates[d.file.path] : true"
            :class="{ 'mt-2 border-t border-border pt-2': i > 0 }"
            @update:open="onToggle(d.file, $event)"
          >
            <div
              class="group flex items-center gap-2 rounded-md px-2 py-1.5"
              :class="{ 'opacity-50': d.hidden }"
            >
              <!-- 多文件时文件名区域可点击折叠;单文件保持静态展示 -->
              <CollapsibleTrigger
                v-if="files.length > 1"
                class="flex min-w-0 flex-1 cursor-pointer items-center gap-2 self-stretch rounded-md text-left hover:bg-accent"
                :title="open ? t('common.collapse') : t('common.expand')"
              >
                <ChevronRight
                  class="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform"
                  :class="{ 'rotate-90': open }"
                />
                <FileCode class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="d.file.path">
                  {{ d.file.path }}
                </span>
              </CollapsibleTrigger>
              <template v-else>
                <FileCode class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="d.file.path">
                  {{ d.file.path }}
                </span>
              </template>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7 shrink-0"
                :class="
                  pinsStore.isPinned(project.id, 'composeFile', d.file.path)
                    ? 'text-yellow-500'
                    : 'hidden group-hover:inline-flex'
                "
                :title="
                  pinsStore.isPinned(project.id, 'composeFile', d.file.path)
                    ? t('pins.unmark')
                    : t('pins.mark')
                "
                @click="toggleFilePin(d.file)"
              >
                <Star
                  class="h-3.5 w-3.5"
                  :class="{
                    'fill-yellow-400': pinsStore.isPinned(project.id, 'composeFile', d.file.path),
                  }"
                />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7 shrink-0"
                :class="d.hidden ? 'text-muted-foreground' : 'hidden group-hover:inline-flex'"
                :title="d.hidden ? t('common.unhide') : t('docker.hideFile')"
                @click="toggleFileHidden(d.file.path, d.hidden)"
              >
                <Eye v-if="d.hidden" class="h-3.5 w-3.5" />
                <EyeOff v-else class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7 shrink-0 text-emerald-600"
                :title="t('docker.up')"
                @click="run(d.file, 'up -d')"
              >
                <Play class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7 shrink-0 text-red-600"
                :title="t('docker.stop')"
                @click="run(d.file, 'down')"
              >
                <Square class="h-3.5 w-3.5" />
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger as-child>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7 shrink-0 text-muted-foreground"
                    :title="t('docker.more')"
                  >
                    <MoreHorizontal class="h-3.5 w-3.5" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" class="w-40">
                  <DropdownMenuItem class="gap-2 text-xs" @click="run(d.file, 'build')">
                    <Hammer class="h-3.5 w-3.5 text-sky-600" />
                    {{ t("docker.build") }}
                  </DropdownMenuItem>
                  <DropdownMenuItem class="gap-2 text-xs" @click="run(d.file, 'up -d --build')">
                    <Hammer class="h-3.5 w-3.5 text-emerald-600" />
                    {{ t("docker.buildUp") }}
                  </DropdownMenuItem>
                  <DropdownMenuItem class="gap-2 text-xs" @click="run(d.file, 'restart')">
                    <RotateCw class="h-3.5 w-3.5 text-amber-600" />
                    {{ t("docker.restart") }}
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem class="gap-2 text-xs" @click="exportAll(d.file, 'container')">
                    <Download class="h-3.5 w-3.5" />
                    {{ t("docker.exportContainer") }}
                  </DropdownMenuItem>
                  <DropdownMenuItem class="gap-2 text-xs" @click="exportAll(d.file, 'image')">
                    <ImageDown class="h-3.5 w-3.5" />
                    {{ t("docker.exportImage") }}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <CollapsibleContent>
              <div
                v-for="s in d.file.services"
                :key="s.name"
                class="group flex min-h-10 items-center gap-2 rounded-md px-2 py-1.5 pl-7 hover:bg-accent"
              >
                <span
                  class="h-2 w-2 shrink-0 rounded-full"
                  :class="dotClass(d.file, s.name)"
                  :title="stateTitle(d.file, s.name)"
                />
                <span class="min-w-0 truncate font-mono text-sm" :title="s.name">
                  {{ s.name }}
                </span>
                <button
                  v-for="p in s.ports"
                  :key="p.published"
                  class="shrink-0 rounded border border-border px-1 font-mono text-[10px] leading-4 text-sky-600 hover:bg-accent dark:text-sky-400"
                  :title="t('docker.openPort')"
                  @click.stop="openPort(p.published)"
                >
                  {{ p.published }}:{{ p.target }}
                </button>
                <span class="min-w-0 flex-1" />
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 shrink-0"
                  :class="
                    pinsStore.isPinned(project.id, 'composeService', stateKey(d.file, s.name))
                      ? 'text-yellow-500'
                      : 'hidden group-hover:inline-flex'
                  "
                  :title="
                    pinsStore.isPinned(project.id, 'composeService', stateKey(d.file, s.name))
                      ? t('pins.unmark')
                      : t('pins.mark')
                  "
                  @click="toggleServicePin(d.file, s.name)"
                >
                  <Star
                    class="h-3.5 w-3.5"
                    :class="{
                      'fill-yellow-400': pinsStore.isPinned(
                        project.id,
                        'composeService',
                        stateKey(d.file, s.name),
                      ),
                    }"
                  />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 shrink-0 text-emerald-600 hidden group-hover:inline-flex"
                  :title="t('docker.up')"
                  @click="run(d.file, 'up -d', s.name)"
                >
                  <Play class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 shrink-0 text-red-600 hidden group-hover:inline-flex"
                  :title="t('docker.stop')"
                  @click="run(d.file, 'stop', s.name)"
                >
                  <Square class="h-3.5 w-3.5" />
                </Button>
                <DropdownMenu
                  @update:open="(v: boolean) => onMoreOpenChange(stateKey(d.file, s.name), v)"
                >
                  <DropdownMenuTrigger as-child>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7 shrink-0 text-muted-foreground"
                      :class="
                        openMoreKey === stateKey(d.file, s.name)
                          ? 'inline-flex'
                          : 'hidden group-hover:inline-flex'
                      "
                      :title="t('docker.more')"
                    >
                      <MoreHorizontal class="h-3.5 w-3.5" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" class="w-40">
                    <DropdownMenuItem class="gap-2 text-xs" @click="run(d.file, 'build', s.name)">
                      <Hammer class="h-3.5 w-3.5 text-sky-600" />
                      {{ t("docker.build") }}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      class="gap-2 text-xs"
                      @click="run(d.file, 'up -d --build', s.name)"
                    >
                      <Hammer class="h-3.5 w-3.5 text-emerald-600" />
                      {{ t("docker.buildUp") }}
                    </DropdownMenuItem>
                    <DropdownMenuItem class="gap-2 text-xs" @click="run(d.file, 'restart', s.name)">
                      <RotateCw class="h-3.5 w-3.5 text-amber-600" />
                      {{ t("docker.restart") }}
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      class="gap-2 text-xs"
                      @click="exportService(d.file, s.name, 'container')"
                    >
                      <Download class="h-3.5 w-3.5" />
                      {{ t("docker.exportContainer") }}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      class="gap-2 text-xs"
                      @click="exportService(d.file, s.name, 'image')"
                    >
                      <ImageDown class="h-3.5 w-3.5" />
                      {{ t("docker.exportImage") }}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </CollapsibleContent>
          </Collapsible>
        </div>
      </ScrollArea>
    </CardContent>
  </Card>
</template>
