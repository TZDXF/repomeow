<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { LoaderCircle, Save } from "@lucide/vue";
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
import { cmd } from "@/lib/tauri";
import type { McpDialect, McpServerEntry } from "@/types";

/**
 * MCP 服务器表单对话框:按目标方言(claude/codex/gemini/opencode)组装服务器
 * 定义并写回对应 agent 的项目级配置文件。新增时可选目标文件(不存在自动创建);
 * 编辑时保留表单未托管的自定义字段,名称不可改(改键名会留下旧条目)。
 *
 * 方言差异:claude 带 type(stdio/http/sse)+ headers;gemini 远程用 url(SSE)/
 * httpUrl(HTTP);codex 无 type、远程头为 http_headers;opencode type 为
 * local/remote、command 是数组、env 叫 environment。
 */
const props = defineProps<{
  projectPath: string;
  /** 全部可管理目标(含未创建文件),新增时的目标选择数据源 */
  targets: Array<{ path: string; dialect: McpDialect; agents: string[] }>;
  /** 编辑的条目及其所在文件;null = 新增 */
  editing: { file: { path: string; dialect: McpDialect }; entry: McpServerEntry } | null;
}>();
const emit = defineEmits<{ (e: "saved"): void }>();
const open = defineModel<boolean>("open", { required: true });

const { t } = useI18n();

type ServerType = "stdio" | "http" | "sse";
const TYPE_LABEL_KEYS: Record<ServerType, string> = {
  stdio: "aiAssets.mcpForm.type_stdio",
  http: "aiAssets.mcpForm.type_http",
  sse: "aiAssets.mcpForm.type_sse",
};
/** 各方言支持的表单类型 */
const DIALECT_SERVER_TYPES: Record<McpDialect, ServerType[]> = {
  claude: ["stdio", "http", "sse"],
  gemini: ["stdio", "http", "sse"],
  codex: ["stdio", "http"],
  opencode: ["stdio", "http"],
};
/** 各方言由表单托管的键,保存时先清再按类型回填,其余自定义字段保留 */
const MANAGED_KEYS: Record<McpDialect, string[]> = {
  claude: ["type", "command", "args", "env", "url", "headers"],
  gemini: ["command", "args", "env", "url", "httpUrl", "headers"],
  codex: ["command", "args", "env", "url", "http_headers"],
  opencode: ["type", "command", "environment", "url", "headers"],
};

const form = reactive({
  path: "",
  name: "",
  type: "stdio" as ServerType,
  command: "",
  argsText: "",
  envText: "",
  url: "",
  headersText: "",
});
const saving = ref(false);

const dialect = computed<McpDialect>(() => {
  if (props.editing) return props.editing.file.dialect;
  return props.targets.find((target) => target.path === form.path)?.dialect ?? "claude";
});
const serverTypes = computed(() => DIALECT_SERVER_TYPES[dialect.value]);
const envKey = computed(() => (dialect.value === "opencode" ? "environment" : "env"));
const headerKey = computed(() => (dialect.value === "codex" ? "http_headers" : "headers"));

function detectType(d: McpDialect, config: Record<string, unknown>): ServerType {
  if (d === "opencode") {
    return config.type === "remote" ? "http" : "stdio";
  }
  if (d === "gemini") {
    if (typeof config.httpUrl === "string") return "http";
    if (typeof config.url === "string") return "sse";
    return "stdio";
  }
  if (d === "codex") {
    return typeof config.url === "string" ? "http" : "stdio";
  }
  if (
    typeof config.type === "string" &&
    DIALECT_SERVER_TYPES.claude.includes(config.type as ServerType)
  ) {
    return config.type as ServerType;
  }
  return typeof config.url === "string" ? "http" : "stdio";
}

watch(open, (v) => {
  if (!v) {
    return;
  }
  form.path = props.editing?.file.path ?? props.targets[0]?.path ?? ".mcp.json";
  const config = props.editing?.entry.config ?? {};
  form.name = props.editing?.entry.name ?? "";
  form.type = detectType(dialect.value, config);
  if (dialect.value === "opencode" && Array.isArray(config.command)) {
    const argv = config.command.filter((a): a is string => typeof a === "string");
    form.command = argv[0] ?? "";
    form.argsText = argv.slice(1).join("\n");
  } else {
    form.command = typeof config.command === "string" ? config.command : "";
    form.argsText = Array.isArray(config.args)
      ? config.args.filter((a): a is string => typeof a === "string").join("\n")
      : "";
  }
  form.envText = pairsToText(config[envKey.value], "=");
  form.url = typeof config.url === "string" ? config.url : "";
  form.headersText = pairsToText(config[headerKey.value], ":");
});

// 切换目标文件会改变方言,当前类型可能不被新方言支持
watch(dialect, (d) => {
  if (!DIALECT_SERVER_TYPES[d].includes(form.type)) {
    form.type = "stdio";
  }
});

/** 键值对象 → 每行一组的表单文本(env 用 =,headers 用 :) */
function pairsToText(value: unknown, sep: string): string {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return "";
  }
  return Object.entries(value as Record<string, unknown>)
    .filter(([, v]) => typeof v === "string")
    .map(([k, v]) => `${k}${sep}${v}`)
    .join("\n");
}

/** 表单文本 → 键值对象;含缺分隔符/空键的行时返回 null 交由校验提示 */
function textToPairs(text: string, sep: string): Record<string, string> | null {
  const out: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line) {
      continue;
    }
    const idx = line.indexOf(sep);
    if (idx <= 0) {
      return null;
    }
    out[line.slice(0, idx).trim()] = line.slice(idx + sep.length).trim();
  }
  return out;
}

function save() {
  if (saving.value) {
    return;
  }
  const name = form.name.trim();
  if (!name || name === "." || name === ".." || name.includes("/") || name.includes("\\")) {
    toast.error(t("aiAssets.mcpForm.invalidName"));
    return;
  }
  if (form.type === "stdio" && !form.command.trim()) {
    toast.error(t("aiAssets.mcpForm.invalidCommand"));
    return;
  }
  if (form.type !== "stdio" && !form.url.trim()) {
    toast.error(t("aiAssets.mcpForm.invalidUrl"));
    return;
  }
  const env = form.type === "stdio" ? textToPairs(form.envText, "=") : {};
  const headers = form.type === "stdio" ? {} : textToPairs(form.headersText, ":");
  if (env === null) {
    toast.error(t("aiAssets.mcpForm.invalidEnv"));
    return;
  }
  if (headers === null) {
    toast.error(t("aiAssets.mcpForm.invalidHeaders"));
    return;
  }
  const d = dialect.value;
  // 编辑时保留表单未托管的自定义字段:从原定义出发,先清托管键再按类型回填
  const config: Record<string, unknown> = { ...(props.editing?.entry.config ?? {}) };
  for (const key of MANAGED_KEYS[d]) {
    delete config[key];
  }
  const args = form.argsText
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  if (form.type === "stdio") {
    if (d === "claude") {
      config.type = "stdio";
      config.command = form.command.trim();
    } else if (d === "opencode") {
      config.type = "local";
      config.command = [form.command.trim(), ...args];
    } else {
      // codex/gemini 无 type,command 为字符串
      config.command = form.command.trim();
    }
    if (d !== "opencode" && args.length) {
      config.args = args;
    }
    if (Object.keys(env).length) {
      config[envKey.value] = env;
    }
  } else {
    if (d === "claude" || d === "opencode") {
      config.type = d === "opencode" ? "remote" : form.type;
    }
    if (d === "gemini") {
      // gemini 远程:SSE 用 url,流式 HTTP 用 httpUrl
      config[form.type === "sse" ? "url" : "httpUrl"] = form.url.trim();
    } else {
      config.url = form.url.trim();
    }
    if (Object.keys(headers).length) {
      config[headerKey.value] = headers;
    }
  }
  void doSave(name, config);
}

async function doSave(name: string, config: Record<string, unknown>) {
  saving.value = true;
  try {
    await cmd("set_project_mcp_server", {
      path: props.projectPath,
      configPath: form.path,
      name,
      config,
    });
    toast.success(t("aiAssets.mcpForm.saved", { name }));
    emit("saved");
    open.value = false;
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-[min(30rem,calc(100%-2rem))]">
      <DialogHeader>
        <DialogTitle>
          {{ editing ? t("aiAssets.mcpForm.editTitle") : t("aiAssets.mcpForm.addTitle") }}
        </DialogTitle>
        <DialogDescription>
          {{ t("aiAssets.mcpForm.description", { path: form.path }) }}
        </DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-4">
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.nameLabel") }}</label>
          <Input
            v-model="form.name"
            :placeholder="t('aiAssets.mcpForm.namePlaceholder')"
            :disabled="!!editing"
          />
        </div>

        <div v-if="!editing" class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.targetLabel") }}</label>
          <Select v-model="form.path">
            <SelectTrigger class="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem v-for="target in targets" :key="target.path" :value="target.path">
                  <span class="font-mono">{{ target.path }}</span>
                  <span class="text-muted-foreground">· {{ target.agents.join(" / ") }}</span>
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
          <p class="text-xs text-muted-foreground">{{ t("aiAssets.mcpForm.targetHint") }}</p>
        </div>

        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.typeLabel") }}</label>
          <div class="flex gap-1 rounded-lg bg-muted p-1">
            <button
              v-for="ty in serverTypes"
              :key="ty"
              type="button"
              class="flex-1 rounded-md px-3 py-1.5 text-sm transition-colors"
              :class="
                form.type === ty
                  ? 'bg-background shadow-sm'
                  : 'text-muted-foreground hover:text-foreground'
              "
              @click="form.type = ty"
            >
              {{ t(TYPE_LABEL_KEYS[ty]) }}
            </button>
          </div>
        </div>

        <template v-if="form.type === 'stdio'">
          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.commandLabel") }}</label>
            <Input
              v-model="form.command"
              class="font-mono"
              :placeholder="t('aiAssets.mcpForm.commandPlaceholder')"
            />
          </div>
          <div v-if="dialect !== 'opencode'" class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.argsLabel") }}</label>
            <Textarea
              v-model="form.argsText"
              rows="3"
              class="font-mono text-xs"
              :placeholder="t('aiAssets.mcpForm.argsPlaceholder')"
            />
          </div>
          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.envLabel") }}</label>
            <Textarea
              v-model="form.envText"
              rows="3"
              class="font-mono text-xs"
              :placeholder="t('aiAssets.mcpForm.envPlaceholder')"
            />
          </div>
        </template>
        <template v-else>
          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.urlLabel") }}</label>
            <Input
              v-model="form.url"
              class="font-mono"
              :placeholder="t('aiAssets.mcpForm.urlPlaceholder')"
            />
          </div>
          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.headersLabel") }}</label>
            <Textarea
              v-model="form.headersText"
              rows="3"
              class="font-mono text-xs"
              :placeholder="t('aiAssets.mcpForm.headersPlaceholder')"
            />
          </div>
        </template>
      </div>

      <DialogFooter>
        <Button variant="outline" :disabled="saving" @click="open = false">
          {{ t("common.cancel") }}
        </Button>
        <Button :disabled="saving" @click="save">
          <LoaderCircle v-if="saving" class="h-3.5 w-3.5 animate-spin" />
          <Save v-else class="h-3.5 w-3.5" />
          {{ saving ? t("common.saving") : t("common.save") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
