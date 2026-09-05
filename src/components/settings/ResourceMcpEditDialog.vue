<script setup lang="ts">
import { ref, watch } from "vue";
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
  formatArgLines,
  formatEnvLines,
  formatHeaderLines,
  parseArgLines,
  parseEnvLines,
  parseHeaderLines,
  updateResourceMcpServer,
  type ResourceMcpServer,
  type ResourceMcpServerInput,
  type ResourceMcpTransport,
} from "@/lib/resource-library";

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

const name = ref("");
const description = ref("");
const transport = ref<ResourceMcpTransport>("stdio");
const command = ref("");
const argsText = ref("");
const envText = ref("");
const url = ref("");
const headersText = ref("");
const saving = ref(false);

watch(
  () => [props.open, props.server] as const,
  ([open]) => {
    if (!open) {
      return;
    }
    // 打开(含编辑 A → 直接切新建)时重置全部表单,清空上次残留
    const server = props.server;
    name.value = server?.name ?? "";
    description.value = server?.description ?? "";
    transport.value = server?.transport ?? "stdio";
    command.value = server?.command ?? "";
    argsText.value = formatArgLines(server?.args ?? []);
    envText.value = formatEnvLines(server?.env ?? {});
    url.value = server?.url ?? "";
    headersText.value = formatHeaderLines(server?.headers ?? {});
  },
);

async function save() {
  const trimmed = name.value.trim();
  if (!trimmed || saving.value) {
    return;
  }
  const input: ResourceMcpServerInput = {
    name: trimmed,
    description: description.value.trim() || undefined,
    transport: transport.value,
  };
  if (transport.value === "stdio") {
    if (!command.value.trim()) {
      toast.error(t("settings.resources.mcp.editDialog.missingCommand"));
      return;
    }
    input.command = command.value.trim();
    input.args = parseArgLines(argsText.value);
    input.env = parseEnvLines(envText.value);
  } else {
    if (!url.value.trim()) {
      toast.error(t("settings.resources.mcp.editDialog.invalidUrl"));
      return;
    }
    input.url = url.value.trim();
    input.headers = parseHeaderLines(headersText.value);
  }
  saving.value = true;
  try {
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
    <DialogContent class="sm:max-w-md">
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
          <div class="flex flex-col gap-1">
            <label class="text-xs text-muted-foreground">
              {{ t("settings.resources.mcp.editDialog.argsLabel") }}
            </label>
            <Textarea
              v-model="argsText"
              rows="3"
              spellcheck="false"
              class="resize-y font-mono text-xs"
              :placeholder="t('settings.resources.mcp.editDialog.argsPlaceholder')"
            />
          </div>
          <div class="flex flex-col gap-1">
            <label class="text-xs text-muted-foreground">
              {{ t("settings.resources.mcp.editDialog.envLabel") }}
            </label>
            <Textarea
              v-model="envText"
              rows="3"
              spellcheck="false"
              class="resize-y font-mono text-xs"
              :placeholder="t('settings.resources.mcp.editDialog.envPlaceholder')"
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
            />
          </div>
          <div class="flex flex-col gap-1">
            <label class="text-xs text-muted-foreground">
              {{ t("settings.resources.mcp.editDialog.headersLabel") }}
            </label>
            <Textarea
              v-model="headersText"
              rows="3"
              spellcheck="false"
              class="resize-y font-mono text-xs"
              :placeholder="t('settings.resources.mcp.editDialog.headersPlaceholder')"
            />
          </div>
        </template>
      </div>

      <DialogFooter>
        <Button variant="outline" :disabled="saving" @click="emit('update:open', false)">
          {{ t("common.cancel") }}
        </Button>
        <Button :disabled="!name.trim() || saving" @click="save">
          {{ t("common.save") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
