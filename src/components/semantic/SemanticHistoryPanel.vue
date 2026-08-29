<script setup lang="ts">
import { computed, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { Loader2, RefreshCw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useSemanticRequest } from "@/composables/useSemanticRequest";
import { cmd } from "@/lib/tauri";
import type { SemanticEntityLogResult, SemanticEntityRef } from "@/types";

// 实体演进时间线(sem log):变化类型、结构变化标记、提交主题、作者、日期、短 hash;
// 点击历史提交跳转 GitGraph(?commit=<fullOid>)定位。

const props = defineProps<{
  /** 项目 ID(GitGraph 路由跳转用) */
  projectId: number;
  root: string;
  entity: SemanticEntityRef | null;
}>();

const open = defineModel<boolean>("open", { required: true });

const { t, te } = useI18n();
const router = useRouter();

const request = useSemanticRequest((requestId: string) => {
  const entity = props.entity;
  if (!entity) return Promise.reject(new Error("no entity"));
  return cmd<SemanticEntityLogResult>("semantic_entity_log", {
    path: props.root,
    entityName: entity.name,
    filePath: entity.filePath || undefined,
    limit: 50,
    requestId,
  });
});

watch([open, () => props.entity], ([isOpen]) => {
  if (isOpen && props.entity) void request.run();
  else request.cancel();
});

const entityMissing = computed(() => request.errorCode.value === "semantic_entity_not_found");

const CHANGE_MARKS: Record<string, string> = {
  added: "+",
  modified: "M",
  deleted: "−",
  renamed: "R",
  moved: "↗",
};

function changeMark(changeType: string): string {
  return CHANGE_MARKS[changeType] ?? "•";
}

function changeLabel(changeType: string): string {
  const key = `git.graph.semantic.change.${changeType}`;
  return te(key) ? t(key) : changeType;
}

/** 提交主题只取首行(完整 message 可能很长) */
function subject(message: string): string {
  return message.split("\n", 1)[0];
}

function shortSha(sha: string): string {
  return sha.slice(0, 7);
}

function gotoCommit(sha: string) {
  open.value = false;
  void router.push({
    name: "project-graph",
    params: { id: props.projectId },
    query: { commit: sha },
  });
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="flex max-h-[85vh] flex-col sm:max-w-2xl">
      <DialogHeader class="shrink-0">
        <DialogTitle class="flex items-center gap-2 pr-8 text-sm">
          <span class="min-w-0 flex-1 truncate font-mono">{{ entity?.name }}</span>
          <span v-if="entity" class="shrink-0 text-xs font-normal text-muted-foreground">
            {{ entity.entityType }} · {{ entity.filePath }}
          </span>
        </DialogTitle>
      </DialogHeader>

      <div
        v-if="request.loading.value"
        class="flex flex-1 items-center justify-center gap-2 py-10 text-sm text-muted-foreground"
      >
        <Loader2 class="h-4 w-4 animate-spin" />
        {{ t("common.loading") }}
      </div>

      <p
        v-else-if="entityMissing"
        class="flex-1 px-1 py-10 text-center text-sm text-muted-foreground"
      >
        {{ t("files.semantic.historyEmpty") }}
      </p>

      <div v-else-if="request.error.value" class="flex flex-1 flex-col items-center gap-2 py-10">
        <p class="whitespace-pre-line text-xs text-destructive">{{ request.error.value }}</p>
        <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="request.run()">
          <RefreshCw class="h-3 w-3" />
          {{ t("common.retry") }}
        </Button>
      </div>

      <p
        v-else-if="!request.result.value?.changes.length"
        class="flex-1 px-1 py-10 text-center text-sm text-muted-foreground"
      >
        {{ t("files.semantic.historyEmpty") }}
      </p>

      <ScrollArea v-else class="min-h-0 flex-1">
        <div class="py-1">
          <button
            v-for="change in request.result.value.changes"
            :key="change.commitSha + change.changeType"
            type="button"
            class="flex w-full items-start gap-2 px-3 py-1.5 text-left hover:bg-accent/60"
            :title="change.commitSha"
            @click="gotoCommit(change.commitSha)"
          >
            <span
              class="w-3 shrink-0 pt-px text-center font-mono text-xs font-semibold text-muted-foreground"
            >
              {{ changeMark(change.changeType) }}
            </span>
            <span class="min-w-0 flex-1">
              <span class="block truncate text-xs">{{ subject(change.message) }}</span>
              <span class="block text-[10px] text-muted-foreground">
                {{ changeLabel(change.changeType) }}
                <template v-if="change.structuralChange === true">
                  · {{ t("files.semantic.structural") }}
                </template>
                · {{ change.author }} · {{ change.date }} ·
                <span class="font-mono">{{ shortSha(change.commitSha) }}</span>
              </span>
            </span>
          </button>
        </div>
      </ScrollArea>
    </DialogContent>
  </Dialog>
</template>
