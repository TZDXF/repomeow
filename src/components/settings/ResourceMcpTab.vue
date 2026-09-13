<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Pencil, Plus, Trash2 } from "@lucide/vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  deleteResourceMcpServer,
  listResourceMcpServers,
  type ResourceMcpServer,
} from "@/lib/resource-library";
import ResourceMcpEditDialog from "./ResourceMcpEditDialog.vue";

const { t } = useI18n();

const loading = ref(true);
const servers = ref<ResourceMcpServer[]>([]);
const editDialogOpen = ref(false);
/** null = 新建;非 null = 编辑该服务器 */
const editingServer = ref<ResourceMcpServer | null>(null);
const pendingDelete = ref<ResourceMcpServer | null>(null);

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
    servers.value = await listResourceMcpServers();
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

onMounted(load);

function openCreate() {
  editingServer.value = null;
  editDialogOpen.value = true;
}

function openEdit(server: ResourceMcpServer) {
  editingServer.value = server;
  editDialogOpen.value = true;
}

function askDelete(server: ResourceMcpServer) {
  pendingDelete.value = server;
}

async function confirmDelete() {
  const server = pendingDelete.value;
  if (!server) {
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
</script>

<template>
  <section>
    <div class="flex items-center justify-between gap-2">
      <p class="text-sm text-muted-foreground">
        {{ t("settings.resources.mcp.description") }}
      </p>
      <div class="flex shrink-0 items-center gap-2">
        <Button size="sm" class="h-8 gap-1.5" @click="openCreate">
          <Plus class="h-3.5 w-3.5" />
          {{ t("settings.resources.mcp.create") }}
        </Button>
      </div>
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
            :title="t('common.edit')"
            @click="openEdit(server)"
          >
            <Pencil class="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7 text-destructive"
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
  </section>
</template>
