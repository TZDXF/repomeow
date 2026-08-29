<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { FileCode2, Loader2, RefreshCw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { groupSemanticChanges } from "@/lib/semantic";
import type { SemanticBinaryChange, SemanticChange, SemanticDiffResult } from "@/types";

const props = defineProps<{
  result: SemanticDiffResult | null;
  loading: boolean;
  error: string;
  selectedPath: string | null;
}>();

const emit = defineEmits<{
  select: [filePath: string, oldFilePath: string | null];
  retry: [];
}>();

const { t, te } = useI18n();
const groups = computed(() => groupSemanticChanges(props.result?.changes ?? []));

function changeLabel(type: string) {
  const key = `git.graph.semantic.change.${type}`;
  return te(key) ? t(key) : type;
}

function changeMark(type: string) {
  return (
    {
      added: "+",
      modified: "M",
      deleted: "−",
      moved: "↗",
      renamed: "R",
      reordered: "↕",
      binary: "B",
    }[type] ?? "•"
  );
}

function changeClass(type: string) {
  return (
    {
      added: "text-green-600 dark:text-green-400",
      modified: "text-amber-600 dark:text-amber-400",
      deleted: "text-red-600 dark:text-red-400",
      moved: "text-violet-600 dark:text-violet-400",
      renamed: "text-blue-600 dark:text-blue-400",
      reordered: "text-cyan-600 dark:text-cyan-400",
      binary: "text-muted-foreground",
    }[type] ?? "text-muted-foreground"
  );
}

function selectChange(change: SemanticChange) {
  emit("select", change.filePath, change.oldFilePath);
}

function selectBinary(change: SemanticBinaryChange) {
  emit("select", change.filePath, change.oldFilePath);
}
</script>

<template>
  <div v-if="loading" class="flex h-full items-center justify-center">
    <Loader2 class="h-4 w-4 animate-spin text-muted-foreground" />
  </div>
  <div v-else-if="error" class="flex flex-col items-start gap-2 px-3 py-2">
    <p class="text-xs text-destructive">{{ error }}</p>
    <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="emit('retry')">
      <RefreshCw class="h-3 w-3" />
      {{ t("common.retry") }}
    </Button>
  </div>
  <p
    v-else-if="!result || (!result.changes.length && !result.binaryChanges.length)"
    class="px-3 py-2 text-xs text-muted-foreground"
  >
    {{ t("git.graph.semantic.empty") }}
  </p>
  <div v-else>
    <div
      class="flex flex-wrap gap-x-2 gap-y-1 border-b px-3 py-1.5 text-[10px] text-muted-foreground"
    >
      <span v-if="result.summary.added" class="text-green-600 dark:text-green-400">
        +{{ result.summary.added }}
      </span>
      <span v-if="result.summary.modified" class="text-amber-600 dark:text-amber-400">
        M {{ result.summary.modified }}
      </span>
      <span v-if="result.summary.deleted" class="text-red-600 dark:text-red-400">
        −{{ result.summary.deleted }}
      </span>
      <span v-if="result.summary.renamed">R {{ result.summary.renamed }}</span>
      <span v-if="result.summary.moved">↗ {{ result.summary.moved }}</span>
      <span v-if="result.summary.reordered">↕ {{ result.summary.reordered }}</span>
      <span v-if="result.summary.binary">B {{ result.summary.binary }}</span>
      <span class="ml-auto">sem {{ result.engineVersion }}</span>
    </div>

    <section v-for="group in groups" :key="group.path">
      <div
        class="sticky top-0 z-1 flex items-center gap-1.5 border-y bg-muted/90 px-3 py-1 text-[10px] text-muted-foreground backdrop-blur"
        :title="group.path"
      >
        <FileCode2 class="h-3 w-3 shrink-0" />
        <span class="truncate font-mono">{{ group.path }}</span>
      </div>
      <button
        v-for="change in group.changes"
        :key="change.entityId + ':' + change.changeType"
        type="button"
        class="flex w-full items-center gap-1.5 px-3 py-1 text-left font-mono text-xs transition-colors hover:bg-accent/60"
        :class="selectedPath === change.filePath ? 'bg-accent' : ''"
        :title="`${changeLabel(change.changeType)} · ${change.filePath}:${change.startLine}`"
        @click="selectChange(change)"
      >
        <span
          class="w-3 shrink-0 text-center font-semibold"
          :class="changeClass(change.changeType)"
        >
          {{ changeMark(change.changeType) }}
        </span>
        <span class="min-w-0 flex-1 truncate">
          <span v-if="change.oldEntityName" class="text-muted-foreground line-through">
            {{ change.oldEntityName }}
          </span>
          <span v-if="change.oldEntityName" class="text-muted-foreground"> → </span>
          {{ change.entityName }}
        </span>
        <span class="shrink-0 text-[10px] text-muted-foreground">{{ change.entityType }}</span>
        <span
          v-if="change.structuralChange === false"
          class="shrink-0 rounded bg-muted px-1 text-[9px] text-muted-foreground"
        >
          {{ t("git.graph.semantic.cosmetic") }}
        </span>
        <span v-if="change.startLine" class="shrink-0 text-[10px] text-muted-foreground">
          :{{ change.startLine }}
        </span>
      </button>
    </section>

    <section v-if="result.binaryChanges.length">
      <div
        class="sticky top-0 z-1 flex items-center gap-1.5 border-y bg-muted/90 px-3 py-1 text-[10px] text-muted-foreground backdrop-blur"
      >
        {{ t("git.graph.semantic.binaryFiles") }}
      </div>
      <button
        v-for="change in result.binaryChanges"
        :key="change.filePath"
        type="button"
        class="flex w-full items-center gap-1.5 px-3 py-1 text-left font-mono text-xs transition-colors hover:bg-accent/60"
        :class="selectedPath === change.filePath ? 'bg-accent' : ''"
        @click="selectBinary(change)"
      >
        <span class="w-3 shrink-0 text-center font-semibold" :class="changeClass('binary')">B</span>
        <span class="min-w-0 flex-1 truncate">{{ change.filePath }}</span>
        <span class="shrink-0 text-[10px] text-muted-foreground">{{ change.fileStatus }}</span>
      </button>
    </section>
  </div>
</template>
