<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Ban, Plus, TerminalSquare } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import CommandEditor from "@/components/scripts/CommandEditor.vue";
import ScriptItem from "@/components/scripts/ScriptItem.vue";
import { COMMAND_ICONS } from "@/lib/command-icons";
import { cmd, runInTerminal } from "@/lib/tauri";
import { usePinsStore } from "@/stores/pins";
import { useProjectOverviewStore } from "@/stores/project-overview";
import type { CustomCommand, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const pinsStore = usePinsStore();
const overviewStore = useProjectOverviewStore();

const commands = ref<CustomCommand[]>([]);

const dialogOpen = ref(false);
const editingId = ref<number | null>(null);
const formName = ref("");
const formCommand = ref("");
const formDescription = ref("");
const formIcon = ref("");
const submitting = ref(false);

async function load() {
  // 聚合 store 与 PackageScripts / DockerCompose 共享一次 get_project_overview IPC,
  // 内部已做失败兜底(返回空数据),失败时卡片按无命令显示
  const overview = await overviewStore.refresh(props.project.id);
  commands.value = overview.custom_commands;
}

watch(() => props.project.id, load, { immediate: true });
pinsStore.ensureLoaded();

function openCreate() {
  editingId.value = null;
  formName.value = "";
  formCommand.value = "";
  formDescription.value = "";
  formIcon.value = "";
  dialogOpen.value = true;
}

function openEdit(c: CustomCommand) {
  editingId.value = c.id;
  formName.value = c.name;
  formCommand.value = c.command;
  formDescription.value = c.description;
  formIcon.value = c.icon;
  dialogOpen.value = true;
}

async function submit() {
  if (!formName.value.trim() || !formCommand.value.trim() || submitting.value) return;
  submitting.value = true;
  try {
    const args = {
      name: formName.value.trim(),
      command: formCommand.value.trim(),
      description: formDescription.value.trim(),
      icon: formIcon.value,
    };
    if (editingId.value == null) {
      await cmd("create_custom_command", { projectId: props.project.id, ...args });
      toast.success(t("scripts.custom.created"));
    } else {
      await cmd("update_custom_command", { id: editingId.value, ...args });
      toast.success(t("scripts.custom.updated"));
    }
    dialogOpen.value = false;
    await load();
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}

async function remove(c: CustomCommand) {
  if (!window.confirm(t("scripts.custom.deleteConfirm", { name: c.name }))) return;
  try {
    await cmd("delete_custom_command", { id: c.id });
    await load();
    toast.success(t("scripts.custom.deleted"));
  } catch (e) {
    toast.error(String(e));
  }
}

async function run(c: CustomCommand) {
  try {
    await runInTerminal(props.project, c.command);
    toast.success(t("scripts.custom.started", { name: c.name }));
  } catch (e) {
    toast.error(String(e));
  }
}

/** 切换自定义命令的「常用命令」标记(target_key 为命令 id,后端会在编辑/删除时同步) */
async function togglePin(c: CustomCommand) {
  const key = String(c.id);
  const pinned = pinsStore.isPinned(props.project.id, "customCommand", key);
  try {
    await pinsStore.setPinned(
      props.project.id,
      { kind: "customCommand", targetKey: key, label: c.name, command: c.command },
      !pinned,
    );
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <Card>
    <CardHeader class="pb-3">
      <CardTitle class="flex items-center gap-2 text-sm font-semibold">
        <TerminalSquare class="h-4 w-4" />
        {{ t("scripts.custom.title") }}
      </CardTitle>
      <CardAction>
        <Button size="sm" variant="outline" @click="openCreate">
          <Plus class="h-4 w-4" />
          {{ t("scripts.custom.new") }}
        </Button>
      </CardAction>
    </CardHeader>
    <CardContent>
      <p v-if="!commands.length" class="text-sm text-muted-foreground">
        {{ t("scripts.custom.empty") }}
      </p>
      <ScrollArea v-else class="max-h-[420px]">
        <div class="flex flex-col">
          <ScriptItem
            v-for="c in commands"
            :key="c.id"
            :name="c.name"
            :command="c.command"
            :description="c.description"
            :icon="c.icon"
            editable
            pinnable
            :pinned="pinsStore.isPinned(project.id, 'customCommand', String(c.id))"
            @run="run(c)"
            @edit="openEdit(c)"
            @delete="remove(c)"
            @toggle-pin="togglePin(c)"
          />
        </div>
      </ScrollArea>
    </CardContent>
  </Card>

  <Dialog v-model:open="dialogOpen">
    <!-- 基类 sm:max-w-sm 在 ≥sm 层叠会盖掉普通 max-w-*,宽度必须用 sm: 变体覆盖(同 DailyReportDialog) -->
    <DialogContent class="sm:max-w-[min(34rem,calc(100%-2rem))]">
      <DialogHeader>
        <DialogTitle>{{
          editingId == null ? t("scripts.custom.dialogNew") : t("scripts.custom.dialogEdit")
        }}</DialogTitle>
      </DialogHeader>
      <form class="flex flex-col gap-3" @submit.prevent="submit">
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("scripts.custom.nameLabel") }}</label>
          <Input v-model="formName" :placeholder="t('scripts.custom.namePlaceholder')" />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("scripts.custom.commandLabel") }}</label>
          <CommandEditor
            v-model="formCommand"
            :placeholder="t('scripts.custom.commandPlaceholder')"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("scripts.custom.descriptionLabel") }}</label>
          <Input
            v-model="formDescription"
            :placeholder="t('scripts.custom.descriptionPlaceholder')"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("scripts.custom.iconLabel") }}</label>
          <div class="grid grid-cols-8 gap-1">
            <button
              type="button"
              class="flex h-8 w-8 items-center justify-center rounded-md border text-muted-foreground transition-colors hover:bg-accent"
              :class="
                formIcon === '' ? 'border-primary bg-accent text-foreground' : 'border-transparent'
              "
              :title="t('scripts.custom.noIcon')"
              @click="formIcon = ''"
            >
              <Ban class="h-4 w-4" />
            </button>
            <button
              v-for="i in COMMAND_ICONS"
              :key="i.name"
              type="button"
              class="flex h-8 w-8 items-center justify-center rounded-md border text-muted-foreground transition-colors hover:bg-accent"
              :class="
                formIcon === i.name
                  ? 'border-primary bg-accent text-foreground'
                  : 'border-transparent'
              "
              :title="i.name"
              @click="formIcon = i.name"
            >
              <component :is="i.component" class="h-4 w-4" />
            </button>
          </div>
        </div>
        <DialogFooter>
          <Button type="submit" :disabled="!formName.trim() || !formCommand.trim() || submitting">
            {{ submitting ? t("common.saving") : t("common.save") }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
