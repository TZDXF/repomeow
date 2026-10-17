<script setup lang="ts">
import Dialog from "@/components/common/ProjectDialog.vue";
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "vue-sonner";
import { FolderOpen } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useProjectsStore } from "@/stores/projects";
import type { Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { required: true });

const store = useProjectsStore();

const path = ref("");
const saving = ref(false);

// 每次打开重置选择,避免残留上次的目录
watch(open, (v) => {
  if (v) path.value = "";
});

async function pickFolder() {
  const selected = await openDialog({
    directory: true,
    multiple: false,
    title: t("projects.relocate.dialogTitle"),
    // 当前路径已失效时不作默认目录,避免系统对话框落到异常位置
    defaultPath: props.project.path_exists ? props.project.path : undefined,
  });
  if (typeof selected === "string") {
    path.value = selected;
  }
}

async function confirm() {
  const target = path.value.trim();
  if (!target || saving.value) return;
  saving.value = true;
  try {
    await store.updateProjectPath(props.project.id, target);
    toast.success(t("projects.relocate.success", { name: props.project.name }));
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
        <DialogTitle>{{ t("projects.relocate.title") }}</DialogTitle>
        <DialogDescription>
          {{ t("projects.relocate.description", { name: project.name }) }}
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
          <label class="text-sm font-medium">{{ t("projects.relocate.pathLabel") }}</label>
          <div class="flex gap-2">
            <Input
              v-model="path"
              readonly
              :placeholder="t('projects.relocate.pathPlaceholder')"
              class="font-mono text-xs"
            />
            <Button type="button" variant="outline" class="shrink-0" @click="pickFolder">
              <FolderOpen class="h-4 w-4" />
              {{ t("projects.relocate.browse") }}
            </Button>
          </div>
        </div>
      </div>

      <DialogFooter>
        <Button variant="outline" @click="open = false">{{ t("common.cancel") }}</Button>
        <Button :disabled="!path.trim() || saving" @click="confirm">
          {{ t("projects.relocate.confirm") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
