<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Loader2, Sparkles } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { generateCommitMessage } from "@/lib/ai";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import type { GitCommitContext, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { required: true });
const store = useProjectsStore();
const settings = useSettingsStore();

const message = ref("");
const submitting = ref(false);
const submittingAndPushing = ref(false);
const generating = ref(false);
// 未跟踪文件默认勾选纳入本次提交
const includeUntracked = ref(true);

const git = computed(() => props.project.git);
const untrackedCount = computed(() => git.value?.untracked ?? 0);
/** 本次实际会提交的变更数(未跟踪文件仅在勾选时计入) */
const committable = computed(() => {
  if (!git.value) return 0;
  return git.value.staged + git.value.modified + (includeUntracked.value ? git.value.untracked : 0);
});

// 每次打开时重置为初始状态
watch(open, (v) => {
  if (v) {
    message.value = "";
    includeUntracked.value = true;
  }
});

async function submit() {
  if (!message.value.trim() || committable.value === 0 || submitting.value) return;
  submitting.value = true;
  try {
    await store.commitChanges(props.project, message.value.trim(), includeUntracked.value);
    toast.success(t("git.commit.success"));
    open.value = false;
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}

async function submitAndPush() {
  if (!message.value.trim() || committable.value === 0 || submitting.value) return;
  submitting.value = true;
  submittingAndPushing.value = true;
  try {
    await store.commitChanges(props.project, message.value.trim(), includeUntracked.value);
    await store.pushRepository(props.project);
    toast.success(t("git.commit.submitAndPushSuccess"));
    open.value = false;
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
    submittingAndPushing.value = false;
  }
}

/** AI 生成提交信息:取暂存+已跟踪修改的 diff 上下文,交给模型后填入输入框 */
async function generate() {
  if (generating.value || committable.value === 0) return;
  generating.value = true;
  try {
    const ctx = await cmd<GitCommitContext>("git_commit_context", { path: props.project.path });
    if (!ctx.stat && !ctx.diff && ctx.untracked.length === 0) return;
    message.value = await generateCommitMessage(ctx, props.project, settings.language);
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  } finally {
    generating.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("git.commit.title") }}</DialogTitle>
        <DialogDescription>{{ t("git.commit.description") }}</DialogDescription>
      </DialogHeader>
      <form class="flex flex-col gap-4" @submit.prevent="submit">
        <div v-if="git" class="flex gap-3 text-xs text-muted-foreground">
          <span>
            {{ t("git.staged") }}
            <span class="font-medium text-emerald-600">{{ git.staged }}</span>
          </span>
          <span>
            {{ t("git.modified") }}
            <span class="font-medium text-amber-600">{{ git.modified }}</span>
          </span>
          <span>
            {{ t("git.untracked") }}
            <span class="font-medium text-sky-600">{{ git.untracked }}</span>
          </span>
        </div>
        <div class="flex flex-col gap-1.5">
          <div class="flex items-center justify-between">
            <label class="text-sm font-medium">{{ t("git.commit.messageLabel") }}</label>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              class="h-7 w-7 text-muted-foreground hover:text-foreground"
              :title="generating ? t('git.commit.generating') : t('git.commit.generate')"
              :disabled="generating || committable === 0"
              @click="generate"
            >
              <Loader2 v-if="generating" class="h-3.5 w-3.5 animate-spin" />
              <Sparkles v-else class="h-3.5 w-3.5" />
            </Button>
          </div>
          <textarea
            v-model="message"
            rows="3"
            :placeholder="t('git.commit.messagePlaceholder')"
            class="w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
            autofocus
            @keydown.enter.ctrl.prevent="submit"
          />
        </div>
        <label
          class="flex w-fit items-center gap-2 text-sm"
          :class="untrackedCount === 0 ? 'cursor-not-allowed opacity-50' : 'cursor-pointer'"
        >
          <input
            v-model="includeUntracked"
            type="checkbox"
            :disabled="untrackedCount === 0"
            class="h-3.5 w-3.5 accent-primary"
          />
          {{ t("git.commit.includeUntracked") }}
          <span class="text-xs text-muted-foreground">({{ untrackedCount }})</span>
        </label>
        <p v-if="committable === 0" class="text-xs text-muted-foreground">
          {{ t("git.commit.empty") }}
        </p>
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            :disabled="!message.trim() || committable === 0 || submitting"
            @click="submitAndPush"
          >
            {{
              submittingAndPushing
                ? t("git.commit.submittingAndPushing")
                : t("git.commit.submitAndPush")
            }}
          </Button>
          <Button type="submit" :disabled="!message.trim() || committable === 0 || submitting">
            {{
              submitting && !submittingAndPushing
                ? t("git.commit.submitting")
                : t("git.actions.commit")
            }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
