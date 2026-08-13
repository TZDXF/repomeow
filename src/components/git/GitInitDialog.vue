<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { FolderGit2, Loader2 } from "@lucide/vue";
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
import type { Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { required: true });
const store = useProjectsStore();

const branch = ref("main");
const remoteUrl = ref("");
const submitting = ref(false);

// 每次打开时重置为初始状态
watch(open, (v) => {
  if (v) {
    branch.value = "main";
    remoteUrl.value = "";
  }
});

async function submit() {
  if (submitting.value) return;
  submitting.value = true;
  try {
    await store.initRepository(props.project, branch.value.trim(), remoteUrl.value.trim());
    toast.success(t("git.init.success"));
    open.value = false;
  } catch (e) {
    // 初始化失败(如远端 URL 无效)保留对话框便于修正重试;git init 幂等,重试无副作用
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("git.init.title") }}</DialogTitle>
        <DialogDescription>{{ t("git.init.description") }}</DialogDescription>
      </DialogHeader>
      <form class="flex flex-col gap-4" @submit.prevent="submit">
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium" for="git-init-branch">
            {{ t("git.init.branchLabel") }}
          </label>
          <Input id="git-init-branch" v-model="branch" placeholder="main" autofocus />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium" for="git-init-remote">
            {{ t("git.init.remoteLabel") }}
          </label>
          <Input
            id="git-init-remote"
            v-model="remoteUrl"
            :placeholder="t('git.init.remotePlaceholder')"
          />
          <p class="text-xs text-muted-foreground">{{ t("git.init.remoteHint") }}</p>
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" :disabled="submitting" @click="open = false">
            {{ t("common.cancel") }}
          </Button>
          <Button type="submit" :disabled="submitting">
            <Loader2 v-if="submitting" class="h-3.5 w-3.5 animate-spin" />
            <FolderGit2 v-else class="h-3.5 w-3.5" />
            {{ submitting ? t("git.init.initializing") : t("git.init.confirm") }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
