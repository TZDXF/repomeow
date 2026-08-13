<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { ChevronRight, Eye, EyeOff, Folder, Package } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { ScrollArea } from "@/components/ui/scroll-area";
import ScriptItem from "@/components/scripts/ScriptItem.vue";
import { useCollapsibleOpen } from "@/composables/useCollapsibleOpen";
import { cmd, runInTerminal } from "@/lib/tauri";
import { usePinsStore } from "@/stores/pins";
import { useProjectOverviewStore } from "@/stores/project-overview";
import { useProjectAssetsStore } from "@/stores/project-assets";
import type { HiddenKind, PackageScript, PackageScriptsGroup, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const pinsStore = usePinsStore();
const assetsStore = useProjectAssetsStore();
const overviewStore = useProjectOverviewStore();

const { isOpen, setOpen } = useCollapsibleOpen("scripts");

/** 扫描结果来自共享 store(与 DockerCompose 卡片合并为一次后端扫描) */
const groups = computed(() => assetsStore.assetsOf(props.project.id)?.package_scripts ?? []);
const loaded = ref(false);
/** 各分组展开状态,key 为分组目录 */
const openStates = ref<Record<string, boolean>>({});
/** 已隐藏的分组(dir)与单条命令("<dir>\n<name>") */
const hiddenGroups = ref<Set<string>>(new Set());
const hiddenScripts = ref<Set<string>>(new Set());
/** 临时显示已隐藏项(灰显,可逐条恢复) */
const showHidden = ref(false);

const scriptKey = (dir: string, name: string) => `${dir}\n${name}`;

const hiddenCount = computed(() => hiddenGroups.value.size + hiddenScripts.value.size);

interface DisplayScript {
  script: PackageScript;
  hidden: boolean;
}
interface DisplayGroup {
  group: PackageScriptsGroup;
  groupHidden: boolean;
  scripts: DisplayScript[];
}

/** 当前应展示的分组:过滤隐藏分组/命令;showHidden 时全部显示但标记 hidden 灰显 */
const displayGroups = computed<DisplayGroup[]>(() =>
  groups.value.flatMap((g) => {
    const groupHidden = hiddenGroups.value.has(g.dir);
    if (groupHidden && !showHidden.value) return [];
    const scripts = g.scripts
      .map((s) => ({
        script: s,
        hidden: groupHidden || hiddenScripts.value.has(scriptKey(g.dir, s.name)),
      }))
      .filter((x) => showHidden.value || !x.hidden);
    if (!scripts.length) return [];
    return [{ group: g, groupHidden, scripts }];
  }),
);

// path 一并作为 watch 源:详情页切换工作区(worktree)时传入的是 id 相同、
// path 不同的 project 副本,需要按新工作目录重新扫描
watch(
  () => [props.project.id, props.project.path],
  async () => {
    loaded.value = false;
    showHidden.value = false;
    pinsStore.ensureLoaded();
    try {
      // refresh 内部去重:与 DockerCompose / CustomCommands 同时挂载只触发一次
      // scan_project_assets / get_project_overview
      const [, overview] = await Promise.all([
        assetsStore.refresh(props.project),
        overviewStore.refresh(props.project.id),
      ]);
      const items = overview.hidden_items;
      hiddenGroups.value = new Set(
        items.filter((i) => i.kind === "packageFile").map((i) => i.targetKey),
      );
      hiddenScripts.value = new Set(
        items.filter((i) => i.kind === "packageScript").map((i) => i.targetKey),
      );
    } catch {
      hiddenGroups.value = new Set();
      hiddenScripts.value = new Set();
    } finally {
      loaded.value = true;
    }
    openStates.value = Object.fromEntries(
      groups.value.map((g) => [
        g.dir,
        isOpen(`${props.project.id}:${g.dir}`, groups.value.length === 1),
      ]),
    );
  },
  { immediate: true },
);

function onToggle(g: PackageScriptsGroup, open: boolean) {
  openStates.value[g.dir] = open;
  setOpen(`${props.project.id}:${g.dir}`, open);
}

function groupLabel(g: PackageScriptsGroup): string {
  return g.dir === "." ? t("scripts.package.rootDir") : g.dir;
}

async function setHidden(kind: HiddenKind, key: string, hidden: boolean) {
  try {
    await cmd("set_hidden_item", { projectId: props.project.id, kind, targetKey: key, hidden });
    const target = kind === "packageFile" ? hiddenGroups : hiddenScripts;
    const next = new Set(target.value);
    if (hidden) next.add(key);
    else next.delete(key);
    target.value = next;
  } catch (e) {
    toast.error(String(e));
  }
}

function toggleGroupHidden(dir: string, hidden: boolean) {
  setHidden("packageFile", dir, !hidden);
}

function toggleScriptHidden(dir: string, name: string, hidden: boolean) {
  setHidden("packageScript", scriptKey(dir, name), !hidden);
}

async function run(group: PackageScriptsGroup, script: PackageScript) {
  // monorepo 子包:在其所在目录内执行 npm run
  const cwd = group.dir === "." ? undefined : `${props.project.path}/${group.dir}`;
  try {
    await runInTerminal(props.project, `npm run ${script.name}`, cwd);
    toast.success(t("scripts.package.started", { name: script.name }));
  } catch (e) {
    toast.error(String(e));
  }
}

/** 切换单条脚本的「常用命令」标记(托盘弹窗中可快速执行) */
async function togglePin(group: PackageScriptsGroup, script: PackageScript) {
  const key = scriptKey(group.dir, script.name);
  const pinned = pinsStore.isPinned(props.project.id, "packageScript", key);
  try {
    await pinsStore.setPinned(
      props.project.id,
      {
        kind: "packageScript",
        targetKey: key,
        label: script.name,
        command: `npm run ${script.name}`,
        // 存相对目录而非绝对路径:项目迁移目录(Relocate)后标记仍可用,执行时再拼接 project.path
        cwd: group.dir === "." ? undefined : group.dir,
      },
      !pinned,
    );
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <!-- 无 package.json(或无 scripts)时整体不渲染卡片,与 DockerCompose 一致;
       全部隐藏时保留头部,以便通过「显示已隐藏」恢复 -->
  <Card v-if="loaded && (displayGroups.length || hiddenCount)" class="group/card">
    <CardHeader class="pb-3">
      <!-- min-h-6 与 hover 才显示的 h-6 按钮同高,避免按钮出现时头部跳动 -->
      <CardTitle class="flex min-h-6 items-center gap-2 text-sm font-semibold">
        <Package class="h-4 w-4" />
        {{ t("scripts.package.title") }}
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
      <ScrollArea class="max-h-[420px]">
        <div class="flex flex-col">
          <Collapsible
            v-for="(d, gi) in displayGroups"
            :key="`${project.id}:${d.group.dir}`"
            v-slot="{ open }"
            :open="groups.length > 1 ? openStates[d.group.dir] : true"
            :class="{ 'mt-2 border-t border-border pt-2': gi > 0 }"
            @update:open="onToggle(d.group, $event)"
          >
            <!-- 多分组时可点击折叠,行尾悬停可隐藏整个 package.json;单分组不显示分组头 -->
            <div
              v-if="groups.length > 1"
              class="group flex min-h-10 items-center gap-1 rounded-md px-2 py-1.5 hover:bg-accent"
              :class="{ 'opacity-50': d.groupHidden }"
            >
              <CollapsibleTrigger
                class="flex min-w-0 flex-1 cursor-pointer items-center gap-2 self-stretch text-left"
                :title="open ? t('common.collapse') : t('common.expand')"
              >
                <ChevronRight
                  class="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform"
                  :class="{ 'rotate-90': open }"
                />
                <Folder class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                <span
                  class="min-w-0 flex-1 truncate font-mono text-xs font-medium"
                  :title="d.group.dir"
                >
                  {{ groupLabel(d.group) }}
                </span>
                <span
                  v-if="d.group.package_name"
                  class="shrink-0 truncate text-xs text-muted-foreground"
                  :title="d.group.package_name"
                >
                  {{ d.group.package_name }}
                </span>
              </CollapsibleTrigger>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7 shrink-0"
                :class="d.groupHidden ? 'text-muted-foreground' : 'hidden group-hover:inline-flex'"
                :title="d.groupHidden ? t('common.unhide') : t('scripts.package.hideFile')"
                @click="toggleGroupHidden(d.group.dir, d.groupHidden)"
              >
                <Eye v-if="d.groupHidden" class="h-3.5 w-3.5" />
                <EyeOff v-else class="h-3.5 w-3.5" />
              </Button>
            </div>
            <CollapsibleContent>
              <ScriptItem
                v-for="x in d.scripts"
                :key="`${d.group.dir}:${x.script.name}`"
                :name="x.script.name"
                :command="x.script.command"
                hidable
                :dimmed="x.hidden"
                pinnable
                :pinned="
                  pinsStore.isPinned(
                    project.id,
                    'packageScript',
                    scriptKey(d.group.dir, x.script.name),
                  )
                "
                @run="run(d.group, x.script)"
                @toggle-hide="toggleScriptHidden(d.group.dir, x.script.name, x.hidden)"
                @toggle-pin="togglePin(d.group, x.script)"
              />
            </CollapsibleContent>
          </Collapsible>
        </div>
      </ScrollArea>
    </CardContent>
  </Card>
</template>
