<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { LoaderCircle } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  addProjectResources,
  loadProjectResources,
  resourceTree,
  selectionState,
  toggleResources,
  type ProjectResourceKind,
  type ProjectResourceSnapshot,
  type ResourceApplyResult,
} from "@/lib/project-ai-resources";

/** 从资源库挑选资源加入项目列表;加入后回到列表逐项配置 Agent。 */
const props = defineProps<{
  projectPath: string;
  kind: ProjectResourceKind;
}>();
const open = defineModel<boolean>("open", { required: true });
const emit = defineEmits<{ changed: [] }>();
const { t } = useI18n();
const router = useRouter();
const data = ref<ProjectResourceSnapshot | null>(null);
const selected = ref(new Set<string>());
const query = ref("");
const loading = ref(false);
const saving = ref(false);
const loadError = ref("");
const failures = ref<ResourceApplyResult["failures"]>([]);
let sequence = 0;
onBeforeUnmount(() => sequence++);

async function load() {
  const seq = ++sequence;
  const path = props.projectPath;
  loading.value = true;
  loadError.value = "";
  try {
    const next = await loadProjectResources(path, props.kind);
    if (seq !== sequence) {
      return;
    }
    data.value = next;
  } catch (e) {
    if (seq === sequence) {
      loadError.value = String(e);
    }
  } finally {
    if (seq === sequence) {
      loading.value = false;
    }
  }
}
watch(
  () => [open.value, props.projectPath, props.kind],
  () => {
    sequence++;
    data.value = null;
    selected.value = new Set();
    query.value = "";
    failures.value = [];
    if (open.value) {
      void load();
    }
  },
);
/** 已在项目列表(shortlist)或已部署的资源不再重复添加。 */
const listedIds = computed(() => {
  const ids = new Set<string>(data.value?.shortlist ?? []);
  for (const d of data.value?.deployments ?? []) {
    ids.add(d.resourceId);
  }
  return ids;
});
const candidates = computed(
  () => data.value?.resources.filter((r) => !listedIds.value.has(r.id)) ?? [],
);
const filtered = computed(() => {
  const q = query.value.trim().toLocaleLowerCase();
  const matchingGroups = new Set(
    data.value?.groups.filter((g) => g.name.toLocaleLowerCase().includes(q)).map((g) => g.id),
  );
  return candidates.value.filter(
    (r) =>
      !q ||
      `${r.name} ${r.description} ${r.source ?? ""}`.toLocaleLowerCase().includes(q) ||
      r.groupIds.some((id) => matchingGroups.has(id)),
  );
});
const tree = computed(() =>
  resourceTree(
    props.kind === "skills" ? (data.value?.groups ?? []) : [],
    filtered.value,
    props.kind === "skills" ? t("projectAi.ungrouped") : "MCP",
  ).filter((g) => g.resources.length),
);
function ids(resources: { id: string }[]) {
  return resources.map((r) => r.id);
}
function toggle(ids: string[], checked: boolean) {
  selected.value = toggleResources(selected.value, ids, checked);
}
async function add() {
  if (saving.value || !data.value || !selected.value.size) {
    return;
  }
  const path = props.projectPath;
  const kind = props.kind;
  saving.value = true;
  failures.value = [];
  try {
    const result = await addProjectResources({
      path,
      kind,
      resourceIds: [...selected.value],
      expectedRevision: data.value.revision,
    });
    if (path !== props.projectPath || kind !== props.kind) {
      return;
    }
    failures.value = result.failures;
    emit("changed");
    if (result.failures.length) {
      toast.error(
        t("projectAi.partial", { count: result.applied, failed: result.failures.length }),
      );
      selected.value = new Set();
      await load();
    } else {
      toast.success(t("projectAi.added", { count: result.applied }));
      open.value = false;
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}
function manage() {
  open.value = false;
  void router.push({ name: "settings", query: { category: "resources" } });
}
</script>

<template>
  <Dialog
    :open="open"
    @update:open="
      (value) => {
        if (!saving) open = value;
      }
    "
  >
    <DialogContent
      class="flex max-h-[85vh] flex-col sm:max-w-2xl"
      @interact-outside="
        (event) => {
          if (saving) event.preventDefault();
        }
      "
      @escape-key-down="
        (event) => {
          if (saving) event.preventDefault();
        }
      "
    >
      <DialogHeader>
        <DialogTitle>{{
          t("projectAi.addTitle", { kind: kind === "skills" ? "Skills" : "MCP" })
        }}</DialogTitle>
        <DialogDescription>{{ t("projectAi.addHint") }}</DialogDescription>
      </DialogHeader>
      <div class="flex items-center gap-2">
        <Input v-model="query" :placeholder="t('projectAi.search')" :disabled="saving || loading" />
      </div>
      <div v-if="loading" class="flex justify-center py-10">
        <LoaderCircle class="size-5 animate-spin" />
      </div>
      <p v-else-if="loadError" role="alert" class="text-sm text-destructive">{{ loadError }}</p>
      <div v-else class="min-h-0 space-y-2 overflow-y-auto">
        <p
          v-if="data?.sourceError"
          role="alert"
          class="rounded-md border border-destructive/30 p-3 text-sm text-destructive"
        >
          {{ t(`errors.${data.sourceError}`) }} · {{ t("projectAi.sourceUnavailableHint") }}
        </p>
        <p
          v-if="!candidates.length && !data?.sourceError"
          class="py-6 text-center text-sm text-muted-foreground"
        >
          {{ data?.resources.length ? t("projectAi.allAdded") : t("projectAi.libraryEmpty") }}
        </p>
        <p
          v-else-if="!filtered.length && candidates.length"
          class="py-6 text-center text-sm text-muted-foreground"
        >
          {{ t("projectAi.noResults") }}
        </p>
        <details v-for="group in tree" :key="group.id" open class="rounded-md border">
          <summary class="cursor-pointer px-3 py-2 text-sm font-medium">
            {{ group.name }}
            <span class="text-muted-foreground">({{ group.resources.length }})</span>
          </summary>
          <label class="flex items-center gap-2 border-t px-3 py-2 text-xs text-muted-foreground">
            <input
              type="checkbox"
              class="accent-primary"
              :checked="selectionState(ids(group.resources), selected) === 'all'"
              :indeterminate="selectionState(ids(group.resources), selected) === 'some'"
              :disabled="saving || !ids(group.resources).length"
              @change="toggle(ids(group.resources), ($event.target as HTMLInputElement).checked)"
            />
            {{ t("projectAi.selectGroup") }}
          </label>
          <label
            v-for="resource in group.resources"
            :key="resource.id"
            class="flex cursor-pointer items-start gap-3 border-t px-3 py-3 hover:bg-accent/50"
          >
            <input
              type="checkbox"
              class="mt-1 accent-primary"
              :checked="selected.has(resource.id)"
              :disabled="saving"
              @change="toggle([resource.id], ($event.target as HTMLInputElement).checked)"
            />
            <div class="min-w-0 flex-1">
              <p class="text-sm font-medium">{{ resource.name }}</p>
              <p
                v-if="resource.description"
                class="mt-1 line-clamp-2 text-xs text-muted-foreground"
              >
                {{ resource.description }}
              </p>
            </div>
          </label>
        </details>
        <div
          v-if="failures.length"
          role="alert"
          class="space-y-2 rounded-md border border-destructive/30 p-3 text-xs text-destructive"
        >
          <p v-for="failure in failures" :key="failure.resourceId" class="break-words">
            {{ failure.resourceId }}:
            {{ t(`errors.${failure.code}`, { context: failure.message }) }}
            <span v-if="failure.code !== 'project_ai_conflict'">{{ failure.message }}</span>
          </p>
        </div>
      </div>
      <DialogFooter class="mt-2 flex-wrap items-center gap-2 border-t pt-4 sm:justify-between">
        <Button variant="ghost" size="sm" :disabled="saving" @click="manage">{{
          t("projectAi.manageLibrary")
        }}</Button>
        <Button :disabled="saving || loading || !selected.size || !!loadError" @click="add"
          ><LoaderCircle v-if="saving" class="size-4 animate-spin" />{{
            t("projectAi.addCount", { count: selected.size })
          }}</Button
        >
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
