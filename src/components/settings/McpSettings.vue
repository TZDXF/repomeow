<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import type { Component } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  Copy,
  FileText,
  FolderSearch,
  GitCommitHorizontal,
  LibraryBig,
  ScanSearch,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { cmd } from "@/lib/tauri";
import { copyToClipboard } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";

interface McpServerInfo {
  executable: string;
  args: string[];
}

type McpGroupId = "git" | "wiki" | "sem" | "project" | "report";

const { t } = useI18n();
const settings = useSettingsStore();
const serverInfo = ref<McpServerInfo | null>(null);
const loadingInfo = ref(true);
const pending = ref<Partial<Record<McpGroupId, boolean>>>({});

const groups: { id: McpGroupId; icon: Component; tools: string[] }[] = [
  { id: "git", icon: GitCommitHorizontal, tools: ["commit_code", "get_git_status"] },
  {
    id: "wiki",
    icon: LibraryBig,
    tools: ["get_wiki_directory", "list_wiki_pages", "read_wiki_page"],
  },
  { id: "sem", icon: ScanSearch, tools: ["sem_find", "sem_context", "sem_relations", "sem_diff"] },
  {
    id: "project",
    icon: FolderSearch,
    tools: ["read_project_file", "list_reports", "list_custom_commands"],
  },
  { id: "report", icon: FileText, tools: ["generate_report"] },
];

const clientConfig = computed(() => {
  if (!serverInfo.value) {
    return "";
  }
  return JSON.stringify(
    {
      mcpServers: {
        repomeow: {
          command: serverInfo.value.executable,
          args: serverInfo.value.args,
        },
      },
    },
    null,
    2,
  );
});

onMounted(async () => {
  try {
    serverInfo.value = await cmd<McpServerInfo>("get_mcp_server_info");
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  } finally {
    loadingInfo.value = false;
  }
});

function groupEnabled(id: McpGroupId): boolean {
  switch (id) {
    case "git":
      return settings.mcpGitCommitEnabled;
    case "wiki":
      return settings.mcpWikiEnabled;
    case "sem":
      return settings.mcpSemEnabled;
    case "project":
      return settings.mcpProjectEnabled;
    case "report":
      return settings.mcpReportEnabled;
  }
}

async function toggleGroup(id: McpGroupId, enabled: boolean) {
  if (pending.value[id]) {
    return;
  }
  pending.value[id] = true;
  try {
    switch (id) {
      case "git":
        await settings.setMcpGitCommitEnabled(enabled);
        break;
      case "wiki":
        await settings.setMcpWikiEnabled(enabled);
        break;
      case "sem":
        await settings.setMcpSemEnabled(enabled);
        break;
      case "project":
        await settings.setMcpProjectEnabled(enabled);
        break;
      case "report":
        await settings.setMcpReportEnabled(enabled);
        break;
    }
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  } finally {
    pending.value[id] = false;
  }
}
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.mcp.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.mcp.description") }}
    </p>

    <div class="mt-5">
      <h3 class="text-sm font-semibold">{{ t("settings.mcp.toolGroups") }}</h3>
      <p class="mt-1 text-xs text-muted-foreground">
        {{ t("settings.mcp.toolGroupsHint") }}
      </p>

      <div class="mt-3 flex flex-col gap-3">
        <div
          v-for="group in groups"
          :key="group.id"
          class="flex items-center justify-between gap-4 rounded-lg border px-3 py-3"
        >
          <div class="flex min-w-0 items-start gap-3">
            <div class="mt-0.5 rounded-md bg-muted p-2 text-muted-foreground">
              <component :is="group.icon" class="h-4 w-4" />
            </div>
            <div class="min-w-0">
              <p class="text-sm font-medium">{{ t(`settings.mcp.${group.id}.title`) }}</p>
              <p class="mt-0.5 text-xs text-muted-foreground">
                {{ t(`settings.mcp.${group.id}.description`) }}
              </p>
              <code class="mt-1.5 block text-[11px] text-muted-foreground">
                {{ group.tools.join(", ") }}
              </code>
            </div>
          </div>
          <Switch
            class="shrink-0"
            :model-value="groupEnabled(group.id)"
            :disabled="pending[group.id]"
            :title="t(`settings.mcp.${group.id}.title`)"
            @update:model-value="toggleGroup(group.id, $event)"
          />
        </div>
      </div>

      <p class="mt-3 rounded-md bg-muted/60 px-3 py-2 text-xs text-muted-foreground">
        {{ t("settings.mcp.reconnectHint") }}
      </p>
    </div>

    <div class="mt-7 border-t pt-6">
      <h3 class="text-sm font-semibold">{{ t("settings.mcp.configuration") }}</h3>
      <ol class="mt-2 list-decimal space-y-1 pl-5 text-xs text-muted-foreground">
        <li>{{ t("settings.mcp.stepEnable") }}</li>
        <li>{{ t("settings.mcp.stepCopy") }}</li>
        <li>{{ t("settings.mcp.stepReconnect") }}</li>
      </ol>

      <div v-if="clientConfig" class="relative mt-3 rounded-lg border bg-muted/40">
        <Button
          variant="ghost"
          size="sm"
          class="absolute right-2 top-2 h-7 gap-1.5 px-2 text-xs"
          @click="copyToClipboard(clientConfig)"
        >
          <Copy class="h-3.5 w-3.5" />
          {{ t("settings.mcp.copyConfig") }}
        </Button>
        <pre
          class="overflow-x-auto p-4 pr-24 text-xs leading-5"
        ><code>{{ clientConfig }}</code></pre>
      </div>
      <p v-else class="mt-3 rounded-md border px-3 py-3 text-xs text-muted-foreground">
        {{ loadingInfo ? t("settings.mcp.loadingConfig") : t("settings.mcp.configUnavailable") }}
      </p>

      <p class="mt-3 text-xs text-muted-foreground">
        {{ t("settings.mcp.builtinHint") }}
      </p>
    </div>
  </section>
</template>
