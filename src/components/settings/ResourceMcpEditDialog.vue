<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
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
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import {
  RESOURCE_MCP_TRANSPORTS,
  createResourceMcpServer,
  parseResourceMcpJson,
  ResourceMcpJsonError,
  updateResourceMcpServer,
  type ParsedResourceMcpEntry,
  type ResourceMcpServer,
  type ResourceMcpServerInput,
  type ResourceMcpTransport,
} from "@/lib/resource-library";
import ResourceMcpRowsEditor, { type McpRow } from "./ResourceMcpRowsEditor.vue";

const props = defineProps<{
  open: boolean;
  /** null = 新建;非 null = 编辑该服务器(须已解锁) */
  server: ResourceMcpServer | null;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  /** 创建或更新成功后触发(父组件刷新列表) */
  saved: [];
}>();

const { t } = useI18n();

// 示例 JSON 是代码而非文案,且 vue-i18n 会把字面 {} 当插值语法,故放组件常量
const JSON_EXAMPLE = `{
  "blender": {
    "command": "cmd",
    "args": ["/c", "uvx", "blender-mcp"]
  }
}`;

/** 连接定义的编辑方式:表单控件或直接编辑 JSON */
type Mode = "form" | "json";

const MODE_ITEMS: { value: Mode; labelKey: string }[] = [
  { value: "form", labelKey: "settings.resources.mcp.editDialog.modeForm" },
  { value: "json", labelKey: "settings.resources.mcp.editDialog.modeJson" },
];

const mode = ref<Mode>("form");
const name = ref("");
const description = ref("");
const transport = ref<ResourceMcpTransport>("stdio");
const command = ref("");
const url = ref("");
const jsonText = ref("");
/** 进入 JSON 模式时由表单序列化的快照,用于识别「未编辑直接切回表单」 */
const jsonSnapshot = ref("");
const saving = ref(false);

// args/env/headers 行式编辑(替代整段文本框);id 仅作行追踪
let rowSeq = 0;
const argRows = ref<McpRow[]>([]);
const envRows = ref<McpRow[]>([]);
const headerRows = ref<McpRow[]>([]);

function nextRowId(): number {
  rowSeq += 1;
  return rowSeq;
}

function rowsFromArgs(args: string[] | undefined): McpRow[] {
  return (args ?? []).map((value) => ({ id: nextRowId(), key: "", value }));
}

function rowsFromMap(map: Record<string, string> | undefined): McpRow[] {
  return Object.entries(map ?? {}).map(([key, value]) => ({ id: nextRowId(), key, value }));
}

function argsFromRows(rows: McpRow[]): string[] {
  return rows.map((row) => row.value.trim()).filter(Boolean);
}

/** 空键行丢弃,重复键后行覆盖前行(与 parseEnvLines 覆盖语义一致) */
function mapFromRows(rows: McpRow[]): Record<string, string> {
  const map: Record<string, string> = {};
  for (const row of rows) {
    const key = row.key.trim();
    if (key) {
      map[key] = row.value.trim();
    }
  }
  return map;
}

const urlInvalid = computed(() => {
  if (transport.value === "stdio") {
    return false;
  }
  const text = url.value.trim();
  if (!text) {
    // 空值由保存按钮禁用兜底,不重复标红
    return false;
  }
  try {
    const parsed = new URL(text);
    return parsed.protocol !== "http:" && parsed.protocol !== "https:";
  } catch {
    return true;
  }
});

const saveDisabled = computed(() => {
  if (saving.value) {
    return true;
  }
  if (mode.value === "json") {
    // 名称可能由 JSON 的 name 字段回填,保存时再校验
    return !jsonText.value.trim();
  }
  if (!name.value.trim()) {
    return true;
  }
  if (transport.value === "stdio") {
    return !command.value.trim();
  }
  return !url.value.trim() || urlInvalid.value;
});

watch(
  () => [props.open, props.server] as const,
  ([open]) => {
    if (!open) {
      return;
    }
    // 打开(含编辑 A → 直接切新建)时重置全部表单,清空上次残留
    const server = props.server;
    mode.value = "form";
    name.value = server?.name ?? "";
    description.value = server?.description ?? "";
    transport.value = server?.transport ?? "stdio";
    command.value = server?.command ?? "";
    url.value = server?.url ?? "";
    jsonText.value = "";
    jsonSnapshot.value = "";
    argRows.value = rowsFromArgs(server?.args);
    envRows.value = rowsFromMap(server?.env);
    headerRows.value = rowsFromMap(server?.headers);
  },
);

/** 表单当前定义 → JSON 模式文本(不含 name/description,二者由顶部字段承载);
 *  定义尚为空(无 command/url)时返回空串,让 JSON 模式留空展示示例占位,
 *  避免粘贴定义时与残留骨架拼成非法 JSON */
function serializeDefinition(): string {
  if (transport.value === "stdio" && !command.value.trim()) {
    return "";
  }
  if (transport.value !== "stdio" && !url.value.trim()) {
    return "";
  }
  const def: Record<string, unknown> = { type: transport.value };
  if (transport.value === "stdio") {
    def.command = command.value.trim();
    const args = argsFromRows(argRows.value);
    const env = mapFromRows(envRows.value);
    if (args.length) {
      def.args = args;
    }
    if (Object.keys(env).length) {
      def.env = env;
    }
  } else {
    def.url = url.value.trim();
    const headers = mapFromRows(headerRows.value);
    if (Object.keys(headers).length) {
      def.headers = headers;
    }
  }
  return JSON.stringify(def, null, 2);
}

function applyEntryToForm(entry: ParsedResourceMcpEntry): void {
  transport.value = entry.transport;
  command.value = entry.command ?? "";
  argRows.value = rowsFromArgs(entry.args);
  envRows.value = rowsFromMap(entry.env);
  url.value = entry.url ?? "";
  headerRows.value = rowsFromMap(entry.headers);
  // JSON 内带 name/description 且顶部字段为空时回填
  if (!name.value.trim() && entry.name) {
    name.value = entry.name;
  }
  if (!description.value.trim() && entry.description) {
    description.value = entry.description;
  }
}

/** 解析 JSON 模式文本,失败弹 toast 并返回 null */
function parseJsonEntry(): ParsedResourceMcpEntry | null {
  try {
    const parsed = parseResourceMcpJson(jsonText.value);
    const [first] = parsed.entries;
    if (!first) {
      toast.error(t("settings.resources.mcp.editDialog.jsonNoEntries"));
      return null;
    }
    if (parsed.entries.length > 1) {
      toast.warning(
        t("settings.resources.mcp.editDialog.jsonMultiple", { count: parsed.entries.length }),
      );
    }
    return first;
  } catch (e) {
    if (e instanceof ResourceMcpJsonError) {
      toast.error(
        t(
          e.code === "invalidJson"
            ? "settings.resources.mcp.editDialog.jsonInvalid"
            : "settings.resources.mcp.editDialog.jsonUnrecognized",
        ),
      );
    } else {
      toast.error(String(e));
    }
    return null;
  }
}

function switchMode(next: Mode) {
  if (next === mode.value) {
    return;
  }
  if (next === "json") {
    jsonSnapshot.value = serializeDefinition();
    jsonText.value = jsonSnapshot.value;
    mode.value = "json";
    return;
  }
  // JSON 未改动时直接回表单(空表单序列化结果不可解析属预期,不算错误)
  if (jsonText.value.trim() === jsonSnapshot.value.trim()) {
    mode.value = "form";
    return;
  }
  // 已编辑但解析失败则留在 JSON 模式,避免静默丢弃改动
  const entry = parseJsonEntry();
  if (!entry) {
    return;
  }
  applyEntryToForm(entry);
  mode.value = "form";
}

function buildInput(): ResourceMcpServerInput {
  return {
    name: name.value.trim(),
    description: description.value.trim() || undefined,
    transport: transport.value,
    ...(transport.value === "stdio"
      ? {
          command: command.value.trim(),
          args: argsFromRows(argRows.value),
          env: mapFromRows(envRows.value),
        }
      : { url: url.value.trim(), headers: mapFromRows(headerRows.value) }),
  };
}

async function save() {
  if (saveDisabled.value) {
    return;
  }
  if (mode.value === "json") {
    const entry = parseJsonEntry();
    if (!entry) {
      return;
    }
    applyEntryToForm(entry);
    if (!name.value.trim()) {
      toast.error(t("settings.resources.mcp.editDialog.nameRequired"));
      return;
    }
  }
  saving.value = true;
  try {
    const input = buildInput();
    if (props.server) {
      await updateResourceMcpServer(props.server.id, input);
      toast.success(t("settings.resources.mcp.editDialog.saved"));
    } else {
      await createResourceMcpServer(input);
      toast.success(t("settings.resources.mcp.editDialog.created"));
    }
    emit("saved");
    emit("update:open", false);
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>
          {{
            server
              ? t("settings.resources.mcp.editDialog.editTitle")
              : t("settings.resources.mcp.editDialog.createTitle")
          }}
        </DialogTitle>
        <DialogDescription>
          {{ t("settings.resources.mcp.editDialog.description") }}
        </DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-3 py-1">
        <div class="flex flex-col gap-1">
          <label class="text-xs text-muted-foreground">
            {{ t("settings.resources.mcp.editDialog.nameLabel") }}
          </label>
          <Input
            v-model="name"
            class="h-8 text-xs"
            :placeholder="t('settings.resources.mcp.editDialog.namePlaceholder')"
            spellcheck="false"
          />
        </div>

        <div class="flex flex-col gap-1">
          <label class="text-xs text-muted-foreground">
            {{ t("settings.resources.mcp.editDialog.descriptionLabel") }}
          </label>
          <Input
            v-model="description"
            class="h-8 text-xs"
            :placeholder="t('settings.resources.mcp.editDialog.descriptionPlaceholder')"
            spellcheck="false"
          />
        </div>

        <!-- 连接定义:表单 / JSON 两种编辑方式 -->
        <div class="flex items-center justify-between gap-2">
          <label class="text-xs text-muted-foreground">
            {{ t("settings.resources.mcp.editDialog.definitionLabel") }}
          </label>
          <div class="flex rounded-md border p-0.5">
            <button
              v-for="item in MODE_ITEMS"
              :key="item.value"
              type="button"
              class="rounded-sm px-2 py-0.5 text-xs transition-colors"
              :class="
                mode === item.value
                  ? 'bg-accent text-foreground'
                  : 'text-muted-foreground hover:text-foreground'
              "
              @click="switchMode(item.value)"
            >
              {{ t(item.labelKey) }}
            </button>
          </div>
        </div>

        <template v-if="mode === 'form'">
          <div class="flex flex-col gap-1">
            <label class="text-xs text-muted-foreground">
              {{ t("settings.resources.mcp.editDialog.transportLabel") }}
            </label>
            <Select v-model="transport">
              <SelectTrigger class="h-8 w-full text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem v-for="kind in RESOURCE_MCP_TRANSPORTS" :key="kind" :value="kind">
                    {{ t(`settings.resources.mcp.transports.${kind}`) }}
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
            <p class="text-[11px] text-muted-foreground">
              {{ t(`settings.resources.mcp.editDialog.transportHint.${transport}`) }}
            </p>
          </div>

          <template v-if="transport === 'stdio'">
            <div class="flex flex-col gap-1">
              <label class="text-xs text-muted-foreground">
                {{ t("settings.resources.mcp.editDialog.commandLabel") }}
              </label>
              <Input
                v-model="command"
                class="h-8 text-xs"
                :placeholder="t('settings.resources.mcp.editDialog.commandPlaceholder')"
                spellcheck="false"
              />
            </div>
            <div class="flex flex-col gap-1.5">
              <label class="text-xs text-muted-foreground">
                {{ t("settings.resources.mcp.editDialog.argsLabel") }}
              </label>
              <ResourceMcpRowsEditor
                :rows="argRows"
                single
                :add-label="t('settings.resources.mcp.editDialog.addArg')"
                :value-placeholder="t('settings.resources.mcp.editDialog.argPlaceholder')"
                @update:rows="argRows = $event"
              />
            </div>
            <div class="flex flex-col gap-1.5">
              <label class="text-xs text-muted-foreground">
                {{ t("settings.resources.mcp.editDialog.envLabel") }}
              </label>
              <ResourceMcpRowsEditor
                :rows="envRows"
                :add-label="t('settings.resources.mcp.editDialog.addEnv')"
                :key-placeholder="t('settings.resources.mcp.editDialog.envKeyPlaceholder')"
                :value-placeholder="t('settings.resources.mcp.editDialog.envValuePlaceholder')"
                @update:rows="envRows = $event"
              />
            </div>
          </template>

          <template v-else>
            <div class="flex flex-col gap-1">
              <label class="text-xs text-muted-foreground">
                {{ t("settings.resources.mcp.editDialog.urlLabel") }}
              </label>
              <Input
                v-model="url"
                class="h-8 text-xs"
                :placeholder="t('settings.resources.mcp.editDialog.urlPlaceholder')"
                spellcheck="false"
                :aria-invalid="urlInvalid || undefined"
              />
              <p v-if="urlInvalid" class="text-xs text-destructive">
                {{ t("settings.resources.mcp.editDialog.invalidUrl") }}
              </p>
            </div>
            <div class="flex flex-col gap-1.5">
              <label class="text-xs text-muted-foreground">
                {{ t("settings.resources.mcp.editDialog.headersLabel") }}
              </label>
              <ResourceMcpRowsEditor
                :rows="headerRows"
                :add-label="t('settings.resources.mcp.editDialog.addHeader')"
                :key-placeholder="t('settings.resources.mcp.editDialog.headerKeyPlaceholder')"
                :value-placeholder="t('settings.resources.mcp.editDialog.headerValuePlaceholder')"
                @update:rows="headerRows = $event"
              />
            </div>
          </template>
        </template>

        <template v-else>
          <div class="flex flex-col gap-1">
            <label class="text-xs text-muted-foreground">
              {{ t("settings.resources.mcp.editDialog.jsonLabel") }}
            </label>
            <Textarea
              v-model="jsonText"
              rows="10"
              spellcheck="false"
              class="resize-y font-mono text-xs"
              :placeholder="JSON_EXAMPLE"
            />
            <p class="text-[11px] text-muted-foreground">
              {{ t("settings.resources.mcp.editDialog.jsonHint") }}
            </p>
          </div>
        </template>
      </div>

      <DialogFooter>
        <Button variant="outline" :disabled="saving" @click="emit('update:open', false)">
          {{ t("common.cancel") }}
        </Button>
        <Button :disabled="saveDisabled" @click="save">
          {{ t("common.save") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
