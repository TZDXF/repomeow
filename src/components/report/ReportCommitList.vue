<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { ChevronRight, Loader2 } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { formatCommitTime } from "@/lib/format";
import type { ProjectCommits } from "@/lib/ai";

const props = defineProps<{ commitData: ProjectCommits[]; loading: boolean }>();
const { t } = useI18n();
const openByProject = ref<Record<string, boolean>>({});
const totalCommits = computed(() =>
  props.commitData.reduce((sum, data) => sum + data.commits.length, 0),
);
</script>

<template>
  <div class="flex min-w-0 flex-col gap-1.5">
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-1.5">
        <label class="text-sm font-medium">{{ t("report.commits") }}</label>
        <Loader2 v-if="loading" class="h-3.5 w-3.5 animate-spin text-muted-foreground" />
      </div>
      <Badge v-if="commitData.length" variant="secondary" class="text-xs">
        {{ t("report.commitCount", { count: totalCommits }) }}
      </Badge>
    </div>
    <div v-if="commitData.length" class="overflow-hidden rounded-md border">
      <Collapsible
        v-for="data in commitData"
        :key="data.projectName"
        v-slot="{ open: expanded }"
        :open="openByProject[data.projectName]"
        @update:open="openByProject[data.projectName] = $event"
      >
        <CollapsibleTrigger
          class="flex w-full cursor-pointer items-center gap-2 px-2.5 py-1.5 text-left text-sm hover:bg-accent"
        >
          <ChevronRight
            class="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform"
            :class="{ 'rotate-90': expanded }"
          />
          <span class="min-w-0 flex-1 truncate">{{ data.projectName }}</span>
          <span class="shrink-0 text-xs whitespace-nowrap text-muted-foreground">
            {{
              data.commits.length
                ? t("report.commitCount", { count: data.commits.length })
                : t("report.excludedNoCommits")
            }}
          </span>
        </CollapsibleTrigger>
        <CollapsibleContent class="min-w-0 overflow-hidden">
          <div
            v-if="data.commits.length"
            class="max-h-40 overflow-y-auto overflow-x-hidden border-t"
          >
            <div
              v-for="commit in data.commits"
              :key="commit.hash + commit.date"
              class="flex min-w-0 items-center gap-2 px-3 py-1 text-xs"
            >
              <code class="shrink-0 rounded bg-muted px-1 py-0.5 font-mono text-[11px]">
                {{ commit.hash }}
              </code>
              <span class="min-w-0 flex-1 truncate" :title="commit.subject">
                {{ commit.subject }}
              </span>
              <span
                class="max-w-28 shrink-0 truncate whitespace-nowrap text-muted-foreground"
                :title="commit.author"
              >
                {{ commit.author }}
              </span>
              <span class="shrink-0 whitespace-nowrap text-muted-foreground" :title="commit.date">
                {{ formatCommitTime(commit.date) }}
              </span>
            </div>
          </div>
          <p v-else class="border-t px-3 py-2 text-xs text-muted-foreground">
            {{ t("report.projectNoCommits") }}
          </p>
        </CollapsibleContent>
      </Collapsible>
    </div>
  </div>
</template>
