<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { Bot, FileCode, FileText, LoaderCircle, RefreshCw, Settings2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import AiFileDrawer from "./AiFileDrawer.vue";
import ProjectResourceSection from "./ProjectResourceSection.vue";
import { listProjectAiTargets, type ProjectAiTarget } from "@/lib/project-ai-resources";
import { cmd } from "@/lib/tauri";
import type { Project, ProjectAiAssets } from "@/types";

/** 资源内容仅在全局资源库维护;项目页负责从资源库添加、按 Agent 配置及安全移除。 */
const props = defineProps<{ project: Project }>();
const { t } = useI18n();
const router = useRouter();
const assets = ref<ProjectAiAssets | null>(null);
const targets = ref<ProjectAiTarget[]>([]);
const loading = ref(false);
const revision = ref(0);
const drawerPath = ref<string | null>(null);
const drawerReadOnly = ref(false);
let sequence = 0;
onBeforeUnmount(() => sequence++);
async function load() {
  const seq = ++sequence;
  loading.value = true;
  try {
    const [scan, agents] = await Promise.all([
      cmd<ProjectAiAssets>("scan_project_ai_assets", { path: props.project.path }),
      listProjectAiTargets(),
    ]);
    if (seq !== sequence) {
      return;
    }
    assets.value = scan;
    targets.value = agents;
  } catch (e) {
    if (seq === sequence) {
      toast.error(String(e));
    }
  } finally {
    if (seq === sequence) {
      loading.value = false;
    }
  }
}
watch(
  () => props.project.path,
  () => {
    assets.value = null;
    drawerPath.value = null;
    void load();
  },
  { immediate: true },
);
const files = computed(
  () => assets.value?.files.filter((f) => !targets.value.some((a) => a.mcpPath === f.path)) ?? [],
);
function preview(path: string, readOnly = true) {
  drawerReadOnly.value = readOnly;
  drawerPath.value = path;
}
function refresh() {
  revision.value++;
  void load();
}
/** 资源区变更:先重扫 assets(非托管检测的数据源),完成后再 bump revision 让资源区静默刷新,保证两侧状态一致、不闪烁。 */
async function resourcesChanged() {
  try {
    await load();
  } finally {
    revision.value++;
  }
}
</script>

<template>
  <div class="flex flex-col gap-6 px-6 pb-6 pt-2">
    <div class="flex items-center gap-2">
      <h2 class="flex items-center gap-2 text-sm font-semibold">
        <Bot class="size-4" />{{ t("aiAssets.title")
        }}<LoaderCircle v-if="loading" class="size-3.5 animate-spin text-muted-foreground" />
      </h2>
      <div class="ml-auto flex items-center gap-1.5">
        <Button
          size="sm"
          variant="outline"
          @click="router.push({ name: 'settings', query: { category: 'resources' } })"
          ><Settings2 class="size-4" />{{ t("projectAi.manageLibrary") }}</Button
        >
        <Button
          size="sm"
          variant="ghost"
          :title="t('aiAssets.refresh')"
          :disabled="loading"
          @click="refresh"
          ><RefreshCw class="size-4"
        /></Button>
      </div>
    </div>
    <ProjectResourceSection
      v-if="assets"
      :project-path="project.path"
      kind="skills"
      :targets="targets"
      :assets="assets"
      :revision="revision"
      :from="`/projects/${project.id}`"
      @preview="preview"
      @changed="resourcesChanged"
    />
    <ProjectResourceSection
      v-if="assets"
      :project-path="project.path"
      kind="mcp"
      :targets="targets"
      :assets="assets"
      :revision="revision"
      :from="`/projects/${project.id}`"
      @preview="preview"
      @changed="resourcesChanged"
    />
    <section v-if="files.length">
      <h3 class="mb-2 border-b pb-2 text-xs font-medium text-muted-foreground">
        {{ t("aiAssets.files") }}
      </h3>
      <div class="grid gap-x-6 xl:grid-cols-2">
        <button
          v-for="file in files"
          :key="file.path"
          class="flex items-center gap-2 rounded-md px-2 py-2 text-left hover:bg-accent"
          @click="preview(file.path, file.kind === 'setting')"
        >
          <component
            :is="file.kind === 'setting' ? Settings2 : file.kind === 'rule' ? FileText : FileCode"
            class="size-3.5 shrink-0 text-muted-foreground"
          />
          <span class="min-w-0 truncate font-mono text-xs">{{ file.path }}</span>
        </button>
      </div>
    </section>
  </div>
  <AiFileDrawer
    :root="project.path"
    :rel-path="drawerPath"
    :read-only="drawerReadOnly"
    @close="drawerPath = null"
    @navigate="
      preview($event, !files.some((file) => file.path === $event && file.kind !== 'setting'))
    "
    @saved="load"
  />
</template>
