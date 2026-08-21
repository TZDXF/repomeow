<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { FolderOpen, RotateCcw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  DEFAULT_COMMIT_PROMPT,
  DEFAULT_REPORT_PROMPT,
  DEFAULT_WEEKLY_REPORT_PROMPT,
  DEFAULT_WIKI_OUTLINE_PROMPT,
  DEFAULT_WIKI_PAGE_PROMPT,
  loadAiPrompts,
  openPromptsDir,
  saveAiPrompts,
} from "@/lib/ai-prompts";

const { t } = useI18n();

type PromptId = "commit" | "report" | "weeklyReport" | "wikiOutline" | "wikiPage";

const activePrompt = ref<PromptId>("commit");

// 本地副本,显式保存后才写入 ~/.repomeow/prompts/*.md;空串 = 使用内置默认模板
const commitPrompt = ref("");
const reportPrompt = ref("");
const weeklyReportPrompt = ref("");
const wikiOutlinePrompt = ref("");
const wikiPagePrompt = ref("");

onMounted(async () => {
  try {
    const prompts = await loadAiPrompts();
    commitPrompt.value = prompts.commit;
    reportPrompt.value = prompts.report;
    weeklyReportPrompt.value = prompts.reportWeekly;
    wikiOutlinePrompt.value = prompts.wikiOutline;
    wikiPagePrompt.value = prompts.wikiPage;
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    toast.error(t("settings.prompts.loadFailed", { error: message }));
  }
});

async function save() {
  try {
    await saveAiPrompts({
      commit: commitPrompt.value,
      report: reportPrompt.value,
      reportWeekly: weeklyReportPrompt.value,
      wikiOutline: wikiOutlinePrompt.value,
      wikiPage: wikiPagePrompt.value,
    });
    toast.success(t("settings.prompts.saved"));
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    toast.error(t("settings.prompts.saveFailed", { error: message }));
  }
}

async function openDir() {
  try {
    await openPromptsDir();
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    toast.error(t("settings.prompts.openDirFailed", { error: message }));
  }
}
</script>

<template>
  <section>
    <div class="flex items-start justify-between gap-4">
      <div>
        <h2 class="text-base font-semibold">{{ t("settings.prompts.title") }}</h2>
        <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.prompts.description") }}</p>
      </div>
      <Button size="sm" variant="outline" class="shrink-0 gap-1.5" @click="openDir">
        <FolderOpen class="h-3.5 w-3.5" />
        {{ t("settings.prompts.openDir") }}
      </Button>
    </div>

    <div class="mt-6 flex flex-wrap gap-2 border-b pb-3">
      <Button
        size="sm"
        :variant="activePrompt === 'commit' ? 'default' : 'outline'"
        @click="activePrompt = 'commit'"
      >
        {{ t("settings.prompts.commit") }}
      </Button>
      <Button
        size="sm"
        :variant="activePrompt === 'report' ? 'default' : 'outline'"
        @click="activePrompt = 'report'"
      >
        {{ t("settings.prompts.report") }}
      </Button>
      <Button
        size="sm"
        :variant="activePrompt === 'weeklyReport' ? 'default' : 'outline'"
        @click="activePrompt = 'weeklyReport'"
      >
        {{ t("settings.prompts.weeklyReport") }}
      </Button>
      <Button
        size="sm"
        :variant="activePrompt === 'wikiOutline' ? 'default' : 'outline'"
        @click="activePrompt = 'wikiOutline'"
      >
        {{ t("settings.prompts.wikiOutline") }}
      </Button>
      <Button
        size="sm"
        :variant="activePrompt === 'wikiPage' ? 'default' : 'outline'"
        @click="activePrompt = 'wikiPage'"
      >
        {{ t("settings.prompts.wikiPage") }}
      </Button>
    </div>

    <div class="mt-5 rounded-lg border bg-card p-4">
      <template v-if="activePrompt === 'commit'">
        <div class="flex items-center justify-between gap-2">
          <label class="text-sm font-medium" for="prompt-commit">
            {{ t("settings.prompts.commit") }}
          </label>
          <Button
            size="sm"
            variant="ghost"
            class="h-7 shrink-0 gap-1 px-2 text-xs text-muted-foreground"
            :disabled="!commitPrompt"
            @click="commitPrompt = ''"
          >
            <RotateCcw class="h-3 w-3" />
            {{ t("settings.prompts.reset") }}
          </Button>
        </div>
        <p class="mt-1.5 text-xs text-muted-foreground">
          {{ t("settings.prompts.commitDescription") }}
        </p>
        <Textarea
          id="prompt-commit"
          v-model="commitPrompt"
          :placeholder="DEFAULT_COMMIT_PROMPT"
          rows="18"
          spellcheck="false"
          class="mt-3 min-h-96 resize-y font-mono text-xs"
        />
      </template>

      <template v-else-if="activePrompt === 'report'">
        <div class="flex items-center justify-between gap-2">
          <label class="text-sm font-medium" for="prompt-report">
            {{ t("settings.prompts.report") }}
          </label>
          <Button
            size="sm"
            variant="ghost"
            class="h-7 shrink-0 gap-1 px-2 text-xs text-muted-foreground"
            :disabled="!reportPrompt"
            @click="reportPrompt = ''"
          >
            <RotateCcw class="h-3 w-3" />
            {{ t("settings.prompts.reset") }}
          </Button>
        </div>
        <p class="mt-1.5 text-xs text-muted-foreground">
          {{ t("settings.prompts.reportDescription") }}
        </p>
        <Textarea
          id="prompt-report"
          v-model="reportPrompt"
          :placeholder="DEFAULT_REPORT_PROMPT"
          rows="18"
          spellcheck="false"
          class="mt-3 min-h-96 resize-y font-mono text-xs"
        />
      </template>

      <template v-else-if="activePrompt === 'weeklyReport'">
        <div class="flex items-center justify-between gap-2">
          <label class="text-sm font-medium" for="prompt-report-weekly">
            {{ t("settings.prompts.weeklyReport") }}
          </label>
          <Button
            size="sm"
            variant="ghost"
            class="h-7 shrink-0 gap-1 px-2 text-xs text-muted-foreground"
            :disabled="!weeklyReportPrompt"
            @click="weeklyReportPrompt = ''"
          >
            <RotateCcw class="h-3 w-3" />
            {{ t("settings.prompts.reset") }}
          </Button>
        </div>
        <p class="mt-1.5 text-xs text-muted-foreground">
          {{ t("settings.prompts.weeklyReportDescription") }}
        </p>
        <Textarea
          id="prompt-report-weekly"
          v-model="weeklyReportPrompt"
          :placeholder="DEFAULT_WEEKLY_REPORT_PROMPT"
          rows="18"
          spellcheck="false"
          class="mt-3 min-h-96 resize-y font-mono text-xs"
        />
      </template>

      <template v-else-if="activePrompt === 'wikiOutline'">
        <div class="flex items-center justify-between gap-2">
          <label class="text-sm font-medium" for="prompt-wiki-outline">
            {{ t("settings.prompts.wikiOutline") }}
          </label>
          <Button
            size="sm"
            variant="ghost"
            class="h-7 shrink-0 gap-1 px-2 text-xs text-muted-foreground"
            :disabled="!wikiOutlinePrompt"
            @click="wikiOutlinePrompt = ''"
          >
            <RotateCcw class="h-3 w-3" />
            {{ t("settings.prompts.reset") }}
          </Button>
        </div>
        <p class="mt-1.5 text-xs text-muted-foreground">
          {{ t("settings.prompts.wikiOutlineDescription") }}
        </p>
        <Textarea
          id="prompt-wiki-outline"
          v-model="wikiOutlinePrompt"
          :placeholder="DEFAULT_WIKI_OUTLINE_PROMPT"
          rows="18"
          spellcheck="false"
          class="mt-3 min-h-96 resize-y font-mono text-xs"
        />
      </template>

      <template v-else>
        <div class="flex items-center justify-between gap-2">
          <label class="text-sm font-medium" for="prompt-wiki-page">
            {{ t("settings.prompts.wikiPage") }}
          </label>
          <Button
            size="sm"
            variant="ghost"
            class="h-7 shrink-0 gap-1 px-2 text-xs text-muted-foreground"
            :disabled="!wikiPagePrompt"
            @click="wikiPagePrompt = ''"
          >
            <RotateCcw class="h-3 w-3" />
            {{ t("settings.prompts.reset") }}
          </Button>
        </div>
        <p class="mt-1.5 text-xs text-muted-foreground">
          {{ t("settings.prompts.wikiPageDescription") }}
        </p>
        <Textarea
          id="prompt-wiki-page"
          v-model="wikiPagePrompt"
          :placeholder="DEFAULT_WIKI_PAGE_PROMPT"
          rows="18"
          spellcheck="false"
          class="mt-3 min-h-96 resize-y font-mono text-xs"
        />
      </template>
    </div>

    <div class="mt-5 flex items-center justify-between gap-4 border-t pt-4">
      <p class="text-xs text-muted-foreground">{{ t("settings.prompts.note") }}</p>
      <Button size="sm" class="shrink-0" @click="save">{{ t("common.save") }}</Button>
    </div>
  </section>
</template>
