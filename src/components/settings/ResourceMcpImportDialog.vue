<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Check } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
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
import {
  importResourceMcpJson,
  parseResourceMcpJson,
  ResourceMcpJsonError,
  type ParsedResourceMcpEntry,
  type ResourceMcpServer,
} from "@/lib/resource-library";

const props = defineProps<{
  open: boolean;
  /** 现有服务器列表,用于预览阶段的重名提示(最终以后端跳过为准) */
  servers: ResourceMcpServer[];
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  /** 至少导入一个服务器后触发(父组件刷新列表) */
  saved: [];
}>();

const { t } = useI18n();

// 示例 JSON 是代码而非文案,且 vue-i18n 会把字面 {} 当插值语法,故放组件常量
const JSON_EXAMPLE = `{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem"]
    },
    "remote": {
      "type": "http",
      "url": "https://mcp.example.com/mcp",
      "headers": { "Authorization": "Bearer xxx" }
    }
  }
}`;

interface ImportRow {
  key: number;
  name: string;
  selected: boolean;
  entry: ParsedResourceMcpEntry;
}

const jsonText = ref("");
const rows = ref<ImportRow[]>([]);
/** 是否已执行过一次解析(驱动空结果提示) */
const hasParsed = ref(false);
const importing = ref(false);
let rowKey = 0;

watch(
  () => props.open,
  (open) => {
    if (!open) {
      return;
    }
    // 打开时重置,清空上次粘贴与解析结果
    jsonText.value = "";
    rows.value = [];
    hasParsed.value = false;
    importing.value = false;
  },
);

function runParse() {
  if (!jsonText.value.trim()) {
    return;
  }
  hasParsed.value = true;
  try {
    const parsed = parseResourceMcpJson(jsonText.value);
    rows.value = parsed.entries.map((entry) => ({
      key: rowKey++,
      name: entry.name,
      selected: true,
      entry,
    }));
    for (const skip of parsed.skipped) {
      toast.warning(
        t(
          skip.reason === "unsupported"
            ? "settings.resources.mcp.importDialog.skipUnsupported"
            : "settings.resources.mcp.importDialog.skipInvalid",
          { name: skip.name },
        ),
      );
    }
  } catch (e) {
    if (e instanceof ResourceMcpJsonError) {
      toast.error(
        t(
          e.code === "invalidJson"
            ? "settings.resources.mcp.importDialog.invalidJson"
            : "settings.resources.mcp.importDialog.unrecognized",
        ),
      );
    } else {
      toast.error(String(e));
    }
  }
}

function toggleRow(row: ImportRow) {
  row.selected = !row.selected;
}

const selectedRows = computed(() => rows.value.filter((row) => row.selected));
const allSelected = computed(
  () => rows.value.length > 0 && selectedRows.value.length === rows.value.length,
);

function toggleAll() {
  const next = !allSelected.value;
  for (const row of rows.value) {
    row.selected = next;
  }
}

function endpointSummary(entry: ParsedResourceMcpEntry): string {
  if (entry.transport === "stdio") {
    return [entry.command ?? "", ...(entry.args ?? [])].filter(Boolean).join(" ");
  }
  return entry.url ?? "";
}

/** 重名提示:与现有库重名或批内重名,仅提示;最终以后端跳过结果为准 */
function conflictOf(row: ImportRow): "existing" | "duplicate" | null {
  const name = row.name.trim();
  if (!name) {
    return null;
  }
  if (props.servers.some((server) => server.name === name)) {
    return "existing";
  }
  if (rows.value.some((other) => other.key !== row.key && other.name.trim() === name)) {
    return "duplicate";
  }
  return null;
}

const importDisabled = computed(
  () =>
    importing.value ||
    selectedRows.value.length === 0 ||
    selectedRows.value.some((row) => !row.name.trim()),
);

async function submit() {
  if (importDisabled.value) {
    return;
  }
  importing.value = true;
  try {
    const outcome = await importResourceMcpJson(
      selectedRows.value.map((row) => ({
        name: row.name.trim(),
        description: row.entry.description,
        transport: row.entry.transport,
        ...(row.entry.transport === "stdio"
          ? { command: row.entry.command, args: row.entry.args, env: row.entry.env }
          : { url: row.entry.url, headers: row.entry.headers }),
      })),
    );
    if (outcome.imported.length) {
      toast.success(
        t("settings.resources.mcp.importDialog.imported", { count: outcome.imported.length }),
      );
    }
    for (const skip of outcome.skipped) {
      toast.warning(
        t(
          skip.reason === "conflict"
            ? "settings.resources.mcp.importDialog.skipConflict"
            : "settings.resources.mcp.importDialog.skipInvalid",
          { name: skip.name },
        ),
      );
    }
    if (outcome.imported.length) {
      emit("saved");
      emit("update:open", false);
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    importing.value = false;
  }
}
</script>

<template>
  <!-- 粘贴标准 MCP JSON(mcpServers / servers / 单定义 / 裸键值表),解析预览后批量导入 -->
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>{{ t("settings.resources.mcp.importDialog.title") }}</DialogTitle>
        <DialogDescription>
          {{ t("settings.resources.mcp.importDialog.description") }}
        </DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-3 py-1">
        <div class="flex flex-col gap-1">
          <label class="text-xs text-muted-foreground">
            {{ t("settings.resources.mcp.importDialog.jsonLabel") }}
          </label>
          <Textarea
            v-model="jsonText"
            rows="7"
            spellcheck="false"
            class="resize-y font-mono text-xs"
            :placeholder="JSON_EXAMPLE"
          />
        </div>
        <div class="flex justify-end">
          <Button
            size="sm"
            variant="outline"
            class="h-7 gap-1 text-xs"
            :disabled="!jsonText.trim() || importing"
            @click="runParse"
          >
            {{ t("settings.resources.mcp.importDialog.parse") }}
          </Button>
        </div>

        <p
          v-if="hasParsed && !rows.length"
          class="rounded-md border border-dashed px-3 py-4 text-center text-xs text-muted-foreground"
        >
          {{ t("settings.resources.mcp.importDialog.noEntries") }}
        </p>
        <template v-else-if="rows.length">
          <div class="flex items-center justify-between">
            <span class="text-xs text-muted-foreground">
              {{
                t("settings.resources.mcp.importDialog.selectedCount", {
                  selected: selectedRows.length,
                  total: rows.length,
                })
              }}
            </span>
            <Button variant="ghost" size="sm" class="h-6 px-2 text-xs" @click="toggleAll">
              {{
                allSelected
                  ? t("settings.resources.mcp.importDialog.deselectAll")
                  : t("settings.resources.mcp.importDialog.selectAll")
              }}
            </Button>
          </div>
          <div class="flex max-h-64 flex-col gap-1.5 overflow-y-auto py-1">
            <div
              v-for="row in rows"
              :key="row.key"
              class="hover:bg-accent flex cursor-pointer items-start gap-2 rounded-md border px-2 py-1.5"
              :class="row.selected ? 'border-primary/50 bg-primary/5' : ''"
              @click="toggleRow(row)"
            >
              <span
                class="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border"
                :class="
                  row.selected
                    ? 'border-primary bg-primary text-primary-foreground'
                    : 'border-input'
                "
              >
                <Check v-if="row.selected" class="h-3 w-3" />
              </span>
              <div class="flex min-w-0 flex-1 flex-col gap-1">
                <div class="flex items-center gap-1.5">
                  <Input
                    v-model="row.name"
                    class="h-6 flex-1 px-1.5 text-xs"
                    :placeholder="t('settings.resources.mcp.importDialog.namePlaceholder')"
                    spellcheck="false"
                    @click.stop
                  />
                  <Badge variant="outline" class="shrink-0 text-[10px]">
                    {{ t(`settings.resources.mcp.transports.${row.entry.transport}`) }}
                  </Badge>
                  <Badge
                    v-if="conflictOf(row)"
                    variant="outline"
                    class="shrink-0 border-amber-500/50 text-[10px] text-amber-600 dark:text-amber-500"
                  >
                    {{
                      conflictOf(row) === "existing"
                        ? t("settings.resources.mcp.importDialog.exists")
                        : t("settings.resources.mcp.importDialog.duplicate")
                    }}
                  </Badge>
                </div>
                <code class="truncate text-[11px] text-muted-foreground">
                  {{ endpointSummary(row.entry) }}
                </code>
              </div>
            </div>
          </div>
          <p
            v-if="selectedRows.some((row) => !row.name.trim())"
            class="text-xs text-amber-600 dark:text-amber-500"
          >
            {{ t("settings.resources.mcp.importDialog.nameRequired") }}
          </p>
        </template>
      </div>

      <DialogFooter>
        <Button variant="outline" :disabled="importing" @click="emit('update:open', false)">
          {{ t("common.cancel") }}
        </Button>
        <Button :disabled="importDisabled" @click="submit">
          {{
            t("settings.resources.mcp.importDialog.importAction", { count: selectedRows.length })
          }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
