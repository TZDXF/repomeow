<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Lock, LockOpen, Pencil, Plus, Trash2 } from "@lucide/vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
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
import {
  deleteResourceMcpServer,
  getResourceBackupStatus,
  listResourceMcpServers,
  onResourceBackupStatusChanged,
  unlockResourceBackup,
  type ResourceBackupStatus,
  type ResourceMcpServer,
} from "@/lib/resource-library";
import ResourceMcpEditDialog from "./ResourceMcpEditDialog.vue";

const { t } = useI18n();

const loading = ref(true);
const servers = ref<ResourceMcpServer[]>([]);
const backupStatus = ref<ResourceBackupStatus | null>(null);
/** 资源库整体加密且未解锁:禁用新增/编辑/删除,提供解锁入口 */
const libraryLocked = computed(
  () => backupStatus.value?.encrypted === true && backupStatus.value?.unlocked !== true,
);

const editDialogOpen = ref(false);
/** null = 新建;非 null = 编辑该服务器 */
const editingServer = ref<ResourceMcpServer | null>(null);
const pendingDelete = ref<ResourceMcpServer | null>(null);

const unlockOpen = ref(false);
const unlockPassphrase = ref("");
const unlocking = ref(false);

const deleteConfirmOpen = computed({
  get: () => pendingDelete.value !== null,
  set: (v) => {
    if (!v) {
      pendingDelete.value = null;
    }
  },
});

function endpointSummary(server: ResourceMcpServer): string {
  if (server.transport === "stdio") {
    return [server.command ?? "", ...(server.args ?? [])].filter(Boolean).join(" ");
  }
  return server.url ?? "";
}

async function load() {
  loading.value = true;
  try {
    backupStatus.value = await getResourceBackupStatus();
    servers.value = libraryLocked.value ? [] : await listResourceMcpServers();
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

let disposed = false;
let unlisten: (() => void) | null = null;

onMounted(async () => {
  await load();
  if (disposed) {
    return;
  }
  onResourceBackupStatusChanged((next) => {
    backupStatus.value = next;
  })
    .then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    })
    .catch(() => {
      // 事件订阅失败仅静默:锁状态仍可经重新进入本页刷新
    });
});

onUnmounted(() => {
  disposed = true;
  unlisten?.();
});

function openCreate() {
  if (libraryLocked.value) {
    return;
  }
  editingServer.value = null;
  editDialogOpen.value = true;
}

function openEdit(server: ResourceMcpServer) {
  if (libraryLocked.value) {
    return;
  }
  editingServer.value = server;
  editDialogOpen.value = true;
}

function askDelete(server: ResourceMcpServer) {
  if (libraryLocked.value) {
    return;
  }
  pendingDelete.value = server;
}

async function confirmDelete() {
  const server = pendingDelete.value;
  if (!server) {
    return;
  }
  if (libraryLocked.value) {
    pendingDelete.value = null;
    return;
  }
  try {
    await deleteResourceMcpServer(server.id);
    servers.value = servers.value.filter((s) => s.id !== server.id);
    toast.success(t("settings.resources.mcp.deleted"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    pendingDelete.value = null;
  }
}

function openUnlock() {
  unlockPassphrase.value = "";
  unlockOpen.value = true;
}

async function submitUnlock() {
  if (!unlockPassphrase.value || unlocking.value) {
    return;
  }
  unlocking.value = true;
  try {
    backupStatus.value = await unlockResourceBackup(unlockPassphrase.value);
    unlockOpen.value = false;
    unlockPassphrase.value = "";
    await load();
    toast.success(t("settings.resources.mcp.unlocked"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    unlocking.value = false;
  }
}
</script>

<template>
  <section>
    <div class="flex items-center justify-between gap-2">
      <p class="text-sm text-muted-foreground">
        {{ t("settings.resources.mcp.description") }}
      </p>
      <Button size="sm" class="h-8 shrink-0 gap-1.5" :disabled="libraryLocked" @click="openCreate">
        <Plus class="h-3.5 w-3.5" />
        {{ t("settings.resources.mcp.create") }}
      </Button>
    </div>

    <div
      v-if="libraryLocked"
      class="mt-4 flex items-center justify-between gap-3 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs"
    >
      <span class="flex min-w-0 items-center gap-1.5">
        <Lock class="h-3 w-3 shrink-0" />
        {{ t("settings.resources.mcp.libraryLocked") }}
      </span>
      <Button size="sm" class="h-7 shrink-0 gap-1" @click="openUnlock">
        <LockOpen class="h-3.5 w-3.5" />
        {{ t("settings.resources.mcp.unlock") }}
      </Button>
    </div>

    <p v-if="loading" class="mt-6 text-center text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </p>
    <div v-else-if="servers.length" class="mt-4 flex flex-col gap-2">
      <div
        v-for="server in servers"
        :key="server.id"
        class="group flex items-center gap-3 rounded-lg border px-3 py-2.5"
      >
        <div class="min-w-0 flex-1">
          <div class="flex min-w-0 items-center gap-2">
            <p class="truncate text-sm font-medium">{{ server.name }}</p>
            <Badge variant="outline" class="shrink-0 text-[10px]">
              {{ t(`settings.resources.mcp.transports.${server.transport}`) }}
            </Badge>
          </div>
          <p v-if="server.description" class="mt-0.5 truncate text-xs text-muted-foreground">
            {{ server.description }}
          </p>
          <code class="mt-0.5 block truncate text-[11px] text-muted-foreground">
            {{ endpointSummary(server) }}
          </code>
        </div>

        <span
          class="flex shrink-0 items-center opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100"
        >
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :disabled="libraryLocked"
            :title="t('common.edit')"
            @click="openEdit(server)"
          >
            <Pencil class="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7 text-destructive"
            :disabled="libraryLocked"
            :title="t('common.delete')"
            @click="askDelete(server)"
          >
            <Trash2 class="h-3.5 w-3.5" />
          </Button>
        </span>
      </div>
    </div>
    <p
      v-else
      class="mt-6 rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
    >
      {{ t("settings.resources.mcp.empty") }}
    </p>

    <ResourceMcpEditDialog v-model:open="editDialogOpen" :server="editingServer" @saved="load" />
    <ConfirmDialog
      v-model:open="deleteConfirmOpen"
      :title="t('common.delete')"
      :description="t('settings.resources.mcp.deleteConfirm', { name: pendingDelete?.name })"
      :confirm-text="t('common.delete')"
      destructive
      @confirm="confirmDelete"
    />

    <Dialog v-model:open="unlockOpen">
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{{ t("settings.resources.mcp.unlockDialog.title") }}</DialogTitle>
          <DialogDescription>
            {{ t("settings.resources.mcp.unlockDialog.description") }}
          </DialogDescription>
        </DialogHeader>
        <form class="flex flex-col gap-3 py-1" @submit.prevent="submitUnlock">
          <div class="flex flex-col gap-1">
            <label class="text-xs text-muted-foreground">
              {{ t("settings.resources.mcp.unlockDialog.passphraseLabel") }}
            </label>
            <Input
              v-model="unlockPassphrase"
              type="password"
              class="h-8 text-xs"
              autocomplete="current-password"
              spellcheck="false"
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              :disabled="unlocking"
              @click="unlockOpen = false"
            >
              {{ t("common.cancel") }}
            </Button>
            <Button type="submit" :disabled="!unlockPassphrase || unlocking">
              {{ t("common.confirm") }}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  </section>
</template>
