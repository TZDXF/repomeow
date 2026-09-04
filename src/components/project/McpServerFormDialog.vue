<script setup lang="ts">
import { reactive, ref, watch } from "vue";
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
import { Textarea } from "@/components/ui/textarea";
import { cmd } from "@/lib/tauri";
import type { McpServerEntry } from "@/types";

/**
 * MCP 服务器表单对话框:新增/编辑单个服务器并写回对应的 MCP 配置文件。
 * 类型分本地命令(stdio: command/args/env)与远程(http/sse: url/headers);
 * 编辑时保留服务器定义里表单未托管的自定义字段,名称不可改(改键名会留下旧条目)。
 */
const props = defineProps<{
  projectPath: string;
  /** 目标 MCP 配置文件(仓库相对路径) */
  configPath: string;
  /** 编辑的条目;null = 新增 */
  editing: McpServerEntry | null;
}>();
const emit = defineEmits<{ (e: "saved"): void }>();
const open = defineModel<boolean>("open", { required: true });

const { t } = useI18n();

type ServerType = "stdio" | "http" | "sse";
const SERVER_TYPES: ServerType[] = ["stdio", "http", "sse"];

const form = reactive({
  name: "",
  type: "stdio" as ServerType,
  command: "",
  argsText: "",
  envText: "",
  url: "",
  headersText: "",
});
const saving = ref(false);

/** 从既有定义推断表单类型:优先 type,缺省按是否有 url 判为 http,否则 stdio */
function detectType(config: Record<string, unknown>): ServerType {
  if (typeof config.type === "string" && SERVER_TYPES.includes(config.type as ServerType)) {
    return config.type as ServerType;
  }
  return typeof config.url === "string" ? "http" : "stdio";
}

watch(open, (v) => {
  if (!v) {
    return;
  }
  const config = props.editing?.config ?? {};
  form.name = props.editing?.name ?? "";
  form.type = detectType(config);
  form.command = typeof config.command === "string" ? config.command : "";
  form.argsText = Array.isArray(config.args)
    ? config.args.filter((a): a is string => typeof a === "string").join("\n")
    : "";
  form.envText = pairsToText(config.env, "=");
  form.url = typeof config.url === "string" ? config.url : "";
  form.headersText = pairsToText(config.headers, ":");
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
  // 编辑时保留表单未托管的自定义字段:从原定义出发,先清托管键再按当前类型回填
  const config: Record<string, unknown> = { ...(props.editing?.config ?? {}) };
  for (const key of ["type", "command", "args", "env", "url", "headers"]) {
    delete config[key];
  }
  if (form.type === "stdio") {
    config.type = "stdio";
    config.command = form.command.trim();
    const args = form.argsText
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
    if (args.length) {
      config.args = args;
    }
    if (Object.keys(env).length) {
      config.env = env;
    }
  } else {
    config.type = form.type;
    config.url = form.url.trim();
    if (Object.keys(headers).length) {
      config.headers = headers;
    }
  }
  void doSave(name, config);
}

async function doSave(name: string, config: Record<string, unknown>) {
  saving.value = true;
  try {
    await cmd("set_project_mcp_server", {
      path: props.projectPath,
      configPath: props.configPath,
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
          {{ t("aiAssets.mcpForm.description", { path: configPath }) }}
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

        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("aiAssets.mcpForm.typeLabel") }}</label>
          <div class="flex gap-1 rounded-lg bg-muted p-1">
            <button
              v-for="ty in SERVER_TYPES"
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
              {{ t(`aiAssets.mcpForm.type_${ty}`) }}
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
          <div class="flex flex-col gap-1.5">
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
