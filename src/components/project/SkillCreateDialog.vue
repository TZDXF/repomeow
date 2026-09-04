<script setup lang="ts">
import { reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { LoaderCircle, Save } from "@lucide/vue";
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
import { Textarea } from "@/components/ui/textarea";
import { cmd } from "@/lib/tauri";

/**
 * 新建项目 Skill 对话框:在 .claude/skills/<名称>/ 写入带 frontmatter
 * 的 SKILL.md 模板;名称即目录名,描述写入 frontmatter(可留空)。
 */
const props = defineProps<{ projectPath: string }>();
const emit = defineEmits<{ (e: "saved", dir: string): void }>();
const open = defineModel<boolean>("open", { required: true });

const { t } = useI18n();

const form = reactive({ name: "", description: "" });
const saving = ref(false);

watch(open, (v) => {
  if (v) {
    form.name = "";
    form.description = "";
  }
});

function save() {
  if (saving.value) return;
  const name = form.name.trim();
  if (!name || name === "." || name === ".." || name.includes("/") || name.includes("\\")) {
    toast.error(t("aiAssets.skillForm.invalidName"));
    return;
  }
  void doSave(name);
}

async function doSave(name: string) {
  saving.value = true;
  try {
    const dir = await cmd<string>("create_project_skill", {
      path: props.projectPath,
      name,
      description: form.description.trim(),
    });
    toast.success(t("aiAssets.skillForm.created", { name }));
    emit("saved", dir);
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
    <DialogContent class="sm:max-w-[min(28rem,calc(100%-2rem))]">
      <DialogHeader>
        <DialogTitle>{{ t("aiAssets.skillForm.title") }}</DialogTitle>
        <DialogDescription>{{ t("aiAssets.skillForm.description") }}</DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-4">
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("aiAssets.skillForm.nameLabel") }}</label>
          <Input
            v-model="form.name"
            class="font-mono"
            :placeholder="t('aiAssets.skillForm.namePlaceholder')"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("aiAssets.skillForm.descLabel") }}</label>
          <Textarea
            v-model="form.description"
            rows="3"
            :placeholder="t('aiAssets.skillForm.descPlaceholder')"
          />
        </div>
      </div>

      <DialogFooter>
        <Button variant="outline" :disabled="saving" @click="open = false">
          {{ t("common.cancel") }}
        </Button>
        <Button :disabled="saving" @click="save">
          <LoaderCircle v-if="saving" class="h-3.5 w-3.5 animate-spin" />
          <Save v-else class="h-3.5 w-3.5" />
          {{ saving ? t("common.saving") : t("common.save") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
