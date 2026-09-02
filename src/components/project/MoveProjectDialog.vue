<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "vue-sonner";
import { FolderOpen } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useProjectsStore } from "@/stores/projects";
import { joinPath, splitDirName } from "@/lib/path";
import type { Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { required: true });

const store = useProjectsStore();

const parentDir = ref("");
const dirName = ref("");
const saving = ref(false);

// 每次打开重置为当前位置与目录名
watch(open, (v) => {
  if (!v) return;
  const { parent, name } = splitDirName(props.project.path);
  parentDir.value = parent;
  dirName.value = name;
});

const targetPreview = computed(() => {
  const name = dirName.value.trim();
  if (!parentDir.value || !name) return "";
  return joinPath(parentDir.value, name);
});

async function pickParent() {
  const selected = await openDialog({
    directory: true,
    multiple: false,
    title: t("projects.moveDir.dialogTitle"),
    defaultPath: parentDir.value || undefined,
  });
  if (typeof selected === "string") {
    parentDir.value = selected;
  }
}

async function confirm() {
  if (!parentDir.value || !dirName.value.trim() || saving.value) return;
  saving.value = true;
  try {
    const moved = await store.moveProjectDir(props.project.id, parentDir.value, dirName.value);
    toast.success(t("projects.moveDir.success", { path: moved.path }));
    open.value = false;
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("projects.moveDir.title") }}</DialogTitle>
        <DialogDescription>
          {{ t("projects.moveDir.description", { name: project.name }) }}
        </DialogDescription>
      </DialogHeader>

      <div class="space-y-3">
        <div class="space-y-1.5">
          <label class="text-sm font-medium">{{ t("projects.relocate.currentPath") }}</label>
          <p class="truncate font-mono text-xs text-muted-foreground" :title="project.path">
            {{ project.path }}
          </p>
        </div>
        <div class="space-y-1.5">
          <label class="text-sm font-medium">{{ t("projects.moveDir.parentLabel") }}</label>
          <div class="flex gap-2">
            <Input
              v-model="parentDir"
              readonly
              :placeholder="t('projects.moveDir.parentPlaceholder')"
              class="font-mono text-xs"
            />
            <Button type="button" variant="outline" class="shrink-0" @click="pickParent">
              <FolderOpen class="h-4 w-4" />
              {{ t("projects.moveDir.browse") }}
            </Button>
          </div>
        </div>
        <div class="space-y-1.5">
          <label class="text-sm font-medium">{{ t("projects.moveDir.nameLabel") }}</label>
          <Input v-model="dirName" class="font-mono text-xs" @keydown.enter.prevent="confirm" />
        </div>
        <div v-if="targetPreview" class="space-y-1.5">
          <label class="text-sm font-medium">{{ t("projects.moveDir.preview") }}</label>
          <p class="truncate font-mono text-xs text-muted-foreground" :title="targetPreview">
            {{ targetPreview }}
          </p>
        </div>
      </div>

      <DialogFooter>
        <Button variant="outline" @click="open = false">{{ t("common.cancel") }}</Button>
        <Button :disabled="!targetPreview || saving" @click="confirm">
          {{ saving ? t("projects.moveDir.moving") : t("projects.moveDir.confirm") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
