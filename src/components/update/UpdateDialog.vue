<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useUpdateStore } from "@/stores/update";

const { t } = useI18n();
const store = useUpdateStore();

const open = computed({
  get: () => store.dialogOpen,
  // 下载安装中也可关闭对话框:下载在 store 中继续(后台下载),
  // 进度经标题栏进度环展示,随时可重新打开查看
  set: (v: boolean) => {
    store.dialogOpen = v;
  },
});

const releaseNotes = computed(() => store.update?.body?.trim() || t("update.noNotes"));
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("update.title", { version: store.update?.version ?? "" }) }}</DialogTitle>
        <DialogDescription>
          {{ t("update.currentVersion", { version: store.currentVersion }) }}
        </DialogDescription>
      </DialogHeader>

      <div class="max-h-64 overflow-y-auto rounded-md border bg-muted/40 p-3">
        <p class="whitespace-pre-line text-sm text-muted-foreground">{{ releaseNotes }}</p>
      </div>

      <div v-if="store.status === 'downloading'" class="flex flex-col gap-1.5">
        <div class="h-2 w-full overflow-hidden rounded-full bg-muted">
          <div
            class="h-full rounded-full bg-primary transition-[width] duration-200"
            :style="{ width: `${store.progress}%` }"
          />
        </div>
        <p class="text-xs text-muted-foreground">
          {{ t("update.downloading", { progress: store.progress }) }}
        </p>
      </div>

      <p v-else-if="store.status === 'installed'" class="text-sm text-muted-foreground">
        {{ t("update.installedHint") }}
      </p>
      <p v-else-if="store.status === 'error'" class="text-sm break-all text-destructive">
        {{ t("update.installFailed", { error: store.error }) }}
      </p>

      <DialogFooter>
        <template v-if="store.status === 'installed'">
          <Button variant="outline" @click="open = false">{{ t("update.later") }}</Button>
          <Button @click="store.relaunchApp()">{{ t("update.restartNow") }}</Button>
        </template>
        <template v-else-if="store.status === 'downloading'">
          <Button variant="outline" @click="open = false">
            {{ t("update.backgroundDownload") }}
          </Button>
        </template>
        <template v-else>
          <Button variant="outline" @click="open = false">
            {{ t("common.cancel") }}
          </Button>
          <Button @click="store.downloadAndInstall()">
            {{ t("update.updateNow") }}
          </Button>
        </template>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
