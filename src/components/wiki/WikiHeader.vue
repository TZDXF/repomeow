<script setup lang="ts">
import { useI18n } from "vue-i18n";
import {
  ArrowLeft,
  BookOpenText,
  FolderOpen,
  GitPullRequestArrow,
  LoaderCircle,
  RefreshCw,
  SlidersHorizontal,
  Trash2,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

defineProps<{
  projectName: string;
  generating: boolean;
  elapsedText: string;
  stale: boolean;
  hasData: boolean;
  updating: boolean;
}>();

const emit = defineEmits<{
  back: [];
  update: [];
  editConfig: [];
  regenerate: [];
  openDir: [];
  remove: [];
}>();

const { t } = useI18n();
</script>

<template>
  <header class="flex shrink-0 items-center gap-2 border-b px-4 py-3">
    <Button
      variant="ghost"
      size="icon"
      class="h-8 w-8 shrink-0"
      :title="t('wiki.back')"
      @click="emit('back')"
    >
      <ArrowLeft class="h-4 w-4" />
    </Button>
    <BookOpenText class="h-4 w-4 shrink-0 text-muted-foreground" />
    <span class="min-w-0 flex-1 truncate text-sm font-medium">
      {{ projectName }} · {{ t("wiki.title") }}
    </span>
    <div
      v-if="generating"
      class="flex shrink-0 items-center gap-2 rounded-full border border-primary/20 bg-primary/5 px-2.5 py-1 text-xs text-primary"
    >
      <LoaderCircle class="h-3.5 w-3.5 animate-spin" />
      <span>{{ t("wiki.progress.inProgress") }}</span>
      <span class="text-muted-foreground">·</span>
      <span class="tabular-nums text-muted-foreground">{{ elapsedText }}</span>
    </div>
    <Badge v-if="stale" variant="secondary" :title="t('wiki.staleHint')">
      {{ t("wiki.stale") }}
    </Badge>
    <template v-if="hasData && !generating">
      <Button
        v-if="stale"
        variant="outline"
        size="sm"
        :disabled="updating"
        :title="t('wiki.updateHint')"
        @click="emit('update')"
      >
        <LoaderCircle v-if="updating" class="h-4 w-4 animate-spin" />
        <GitPullRequestArrow v-else class="h-4 w-4" />
        {{ t("wiki.update") }}
      </Button>
      <Button
        variant="outline"
        size="sm"
        :title="t('wiki.genConfigTitle')"
        @click="emit('editConfig')"
      >
        <SlidersHorizontal class="h-4 w-4" />
      </Button>
      <Button variant="outline" size="sm" @click="emit('regenerate')">
        <RefreshCw class="h-4 w-4" />
        {{ t("wiki.regenerate") }}
      </Button>
      <Button variant="outline" size="sm" @click="emit('openDir')">
        <FolderOpen class="h-4 w-4" />
        {{ t("wiki.openDir") }}
      </Button>
      <Button variant="outline" size="sm" @click="emit('remove')">
        <Trash2 class="h-4 w-4" />
        {{ t("wiki.delete") }}
      </Button>
    </template>
  </header>
</template>
