<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { FolderOpen, LoaderCircle, RefreshCw } from "@lucide/vue";
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
import { formatRelativeTime } from "@/lib/format";
import {
  configureResourceBackup,
  getResourceBackupStatus,
  onResourceBackupStatusChanged,
  openResourceLibraryDir,
  resolveResourceBackup,
  syncResourceBackupNow,
  unlinkResourceBackup,
  type ResourceBackupStatus,
} from "@/lib/resource-library";

const { t } = useI18n();

const status = ref<ResourceBackupStatus | null>(null);
const loading = ref(true);
const syncing = ref(false);

const stateVariant = computed(() => {
  const state = status.value?.state;
  if (state === "error") {
    return "destructive";
  }
  if (state === "syncing") {
    return "default";
  }
  return "outline";
});

const stateLabel = computed(() => {
  const state = status.value?.state ?? "never";
  return t(`settings.resources.backup.state.${state}`);
});

let disposed = false;
let unlisten: (() => void) | null = null;

async function load() {
  loading.value = true;
  try {
    status.value = await getResourceBackupStatus();
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

onMounted(async () => {
  await load();
  if (disposed) {
    return;
  }
  onResourceBackupStatusChanged((next) => {
    status.value = next;
  })
    .then((fn) => {
      if (disposed) {
        // 订阅完成前组件已卸载:立即注销,避免泄漏
        fn();
      } else {
        unlisten = fn;
      }
    })
    .catch(() => {
      // 事件订阅失败仅静默:状态仍可经手动操作刷新
    });
  // 打开资源页时的一次非阻塞同步检查(仅已配置远端;结果经事件回填)
  if (status.value?.configured) {
    syncResourceBackupNow()
      .then((next) => {
        if (!disposed) status.value = next;
      })
      .catch(() => {
        // 后台检查失败静默:状态栏保留最近一次同步结果
      });
  }
});

onUnmounted(() => {
  disposed = true;
  unlisten?.();
});

// 配置远端
const configOpen = ref(false);
const remoteUrl = ref("");
const branch = ref("");
const savingConfig = ref(false);

function openConfig() {
  remoteUrl.value = status.value?.remoteUrl ?? "";
  branch.value = status.value?.branch || "main";
  configOpen.value = true;
}

async function saveConfig() {
  const url = remoteUrl.value.trim();
  if (!url || savingConfig.value) {
    return;
  }
  savingConfig.value = true;
  try {
    status.value = await configureResourceBackup(url, branch.value.trim() || undefined);
    configOpen.value = false;
    toast.success(t("settings.resources.backup.configDialog.saved"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    savingConfig.value = false;
  }
}

// 同步与解除
async function syncNow() {
  if (syncing.value) {
    return;
  }
  syncing.value = true;
  try {
    status.value = await syncResourceBackupNow();
    toast.success(t("settings.resources.backup.synced"));
  } catch (e) {
    toast.error(t("settings.resources.backup.syncFailed", { error: String(e) }));
  } finally {
    syncing.value = false;
  }
}

// 通用确认(解除同步 / 分叉解决方向)
type PendingConfirm = "unlink" | "resolveRemote" | "resolveLocal";
const pendingConfirm = ref<PendingConfirm | null>(null);
const confirmOpen = computed({
  get: () => pendingConfirm.value !== null,
  set: (v) => {
    if (!v) {
      pendingConfirm.value = null;
    }
  },
});

const confirmDescription = computed(() => {
  switch (pendingConfirm.value) {
    case "unlink":
      return t("settings.resources.backup.unlinkConfirm");
    case "resolveRemote":
      return t("settings.resources.backup.resolveRemoteConfirm");
    case "resolveLocal":
      return t("settings.resources.backup.resolveLocalConfirm");
    default:
      return "";
  }
});

const confirmDestructive = computed(
  () => pendingConfirm.value === "unlink" || pendingConfirm.value === "resolveRemote",
);

async function confirm() {
  switch (pendingConfirm.value) {
    case "unlink":
      try {
        await unlinkResourceBackup();
        toast.success(t("settings.resources.backup.unlinked"));
        await load();
      } catch (e) {
        toast.error(String(e));
      }
      break;
    case "resolveRemote":
    case "resolveLocal":
      try {
        status.value = await resolveResourceBackup(pendingConfirm.value === "resolveRemote");
      } catch (e) {
        toast.error(String(e));
      }
      break;
  }
  pendingConfirm.value = null;
}

async function openDir() {
  try {
    await openResourceLibraryDir();
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <section v-if="loading && !status" class="py-6 text-center text-xs text-muted-foreground">
    {{ t("common.loading") }}
  </section>
  <section v-else class="flex flex-col gap-4">
    <div class="rounded-lg border p-4">
      <div class="flex items-start justify-between gap-3">
        <div>
          <h3 class="text-sm font-semibold">{{ t("settings.resources.backup.title") }}</h3>
          <p class="mt-1 text-xs text-muted-foreground">
            {{ t("settings.resources.backup.description") }}
          </p>
        </div>
        <Badge v-if="status" :variant="stateVariant" class="shrink-0">{{ stateLabel }}</Badge>
      </div>

      <template v-if="!status?.configured">
        <p
          class="mt-3 rounded-md border border-dashed px-3 py-6 text-center text-xs text-muted-foreground"
        >
          {{ t("settings.resources.backup.notConfigured") }}
        </p>
        <div class="mt-3 flex justify-end gap-2">
          <Button size="sm" variant="outline" class="h-8 gap-1.5" @click="openDir">
            <FolderOpen class="h-3.5 w-3.5" />
            {{ t("settings.resources.backup.openDir") }}
          </Button>
          <Button size="sm" class="h-8" @click="openConfig">
            {{ t("settings.resources.backup.configure") }}
          </Button>
        </div>
      </template>

      <template v-else>
        <dl class="mt-3 grid grid-cols-1 gap-x-6 gap-y-2 text-xs md:grid-cols-2">
          <div class="flex min-w-0 gap-2">
            <dt class="shrink-0 text-muted-foreground">
              {{ t("settings.resources.backup.remoteUrl") }}
            </dt>
            <dd class="min-w-0 truncate" :title="status.remoteUrl">{{ status.remoteUrl }}</dd>
          </div>
          <div class="flex gap-2">
            <dt class="shrink-0 text-muted-foreground">
              {{ t("settings.resources.backup.branch") }}
            </dt>
            <dd>{{ status.branch || "main" }}</dd>
          </div>
          <div class="flex gap-2">
            <dt class="shrink-0 text-muted-foreground">
              {{ t("settings.resources.backup.lastSync") }}
            </dt>
            <dd>
              {{
                status.lastSyncAt
                  ? formatRelativeTime(status.lastSyncAt)
                  : t("settings.resources.backup.never")
              }}
            </dd>
          </div>
          <div v-if="status.ahead || status.behind" class="flex gap-2">
            <dt class="shrink-0 text-muted-foreground">
              {{ t("settings.resources.backup.aheadBehindLabel") }}
            </dt>
            <dd>
              {{
                t("settings.resources.backup.aheadBehind", {
                  ahead: status.ahead ?? 0,
                  behind: status.behind ?? 0,
                })
              }}
            </dd>
          </div>
        </dl>

        <div
          v-if="status.state === 'diverged'"
          class="mt-3 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs"
        >
          <p>{{ t("settings.resources.backup.diverged") }}</p>
          <div class="mt-2 flex gap-2">
            <Button
              size="sm"
              variant="outline"
              class="h-7"
              @click="pendingConfirm = 'resolveRemote'"
            >
              {{ t("settings.resources.backup.resolveRemote") }}
            </Button>
            <Button
              size="sm"
              variant="outline"
              class="h-7"
              @click="pendingConfirm = 'resolveLocal'"
            >
              {{ t("settings.resources.backup.resolveLocal") }}
            </Button>
          </div>
        </div>

        <p
          v-if="status.state === 'error' && status.error"
          class="mt-3 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          {{ status.error }}
        </p>

        <div class="mt-4 flex justify-end gap-2 border-t pt-3">
          <Button size="sm" variant="outline" class="h-8 gap-1.5" @click="openDir">
            <FolderOpen class="h-3.5 w-3.5" />
            {{ t("settings.resources.backup.openDir") }}
          </Button>
          <Button size="sm" variant="outline" class="h-8" @click="pendingConfirm = 'unlink'">
            {{ t("settings.resources.backup.unlink") }}
          </Button>
          <Button size="sm" variant="outline" class="h-8" @click="openConfig">
            {{ t("settings.resources.backup.edit") }}
          </Button>
          <Button size="sm" class="h-8 gap-1.5" :disabled="syncing" @click="syncNow">
            <LoaderCircle v-if="syncing" class="h-3.5 w-3.5 animate-spin" />
            <RefreshCw v-else class="h-3.5 w-3.5" />
            {{
              syncing
                ? t("settings.resources.backup.syncing")
                : t("settings.resources.backup.syncNow")
            }}
          </Button>
        </div>
      </template>
    </div>

    <Dialog v-model:open="configOpen">
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{{ t("settings.resources.backup.configDialog.title") }}</DialogTitle>
          <DialogDescription>
            {{ t("settings.resources.backup.configDialog.description") }}
          </DialogDescription>
        </DialogHeader>
        <form class="flex flex-col gap-3 py-1" @submit.prevent="saveConfig">
          <div class="flex flex-col gap-1">
            <label class="text-xs text-muted-foreground">
              {{ t("settings.resources.backup.configDialog.remoteUrlLabel") }}
            </label>
            <Input
              v-model="remoteUrl"
              class="h-8 text-xs"
              :placeholder="t('settings.resources.backup.configDialog.remoteUrlPlaceholder')"
              spellcheck="false"
            />
          </div>
          <div class="flex flex-col gap-1">
            <label class="text-xs text-muted-foreground">
              {{ t("settings.resources.backup.configDialog.branchLabel") }}
            </label>
            <Input
              v-model="branch"
              class="h-8 text-xs"
              :placeholder="t('settings.resources.backup.configDialog.branchPlaceholder')"
              spellcheck="false"
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              :disabled="savingConfig"
              @click="configOpen = false"
            >
              {{ t("common.cancel") }}
            </Button>
            <Button type="submit" :disabled="!remoteUrl.trim() || savingConfig">
              {{ t("common.save") }}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>

    <ConfirmDialog
      v-model:open="confirmOpen"
      :title="t('common.confirm')"
      :description="confirmDescription"
      :destructive="confirmDestructive"
      @confirm="confirm"
    />
  </section>
</template>
