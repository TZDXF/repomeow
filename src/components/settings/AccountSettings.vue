<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { GitBranch, Loader2, Pencil, Plus, Trash2 } from "@lucide/vue";
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
import { Switch } from "@/components/ui/switch";
import {
  addGitAccount,
  listGitAccounts,
  removeGitAccount,
  updateGitAccount,
  type GitAccount,
  type Provider,
} from "@/lib/accounts";
import { useSettingsStore } from "@/stores/settings";

const { t } = useI18n();
const settingsStore = useSettingsStore();

const accounts = ref<GitAccount[]>([]);
const loading = ref(false);

const enableGhCli = computed({
  get: () => settingsStore.enableGhCli,
  set: (v: boolean) => settingsStore.setEnableGhCli(v),
});

const PROVIDERS: { value: Provider; label: string }[] = [
  { value: "github", label: "GitHub" },
  { value: "gitee", label: "Gitee" },
  { value: "gitlab", label: "GitLab" },
];

function providerLabel(p: Provider): string {
  return PROVIDERS.find((x) => x.value === p)?.label ?? p;
}

async function reload() {
  loading.value = true;
  try {
    accounts.value = await listGitAccounts();
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

onMounted(reload);

// ── 新增 / 编辑对话框 ──────────────────────────────────────────
const dialogOpen = ref(false);
const editing = ref<GitAccount | null>(null);
const provider = ref<Provider>("github");
const label = ref("");
const baseUrl = ref("");
const token = ref("");
const saving = ref(false);

function openAdd() {
  editing.value = null;
  provider.value = "github";
  label.value = "";
  baseUrl.value = "";
  token.value = "";
  dialogOpen.value = true;
}

function openEdit(account: GitAccount) {
  editing.value = account;
  provider.value = account.provider;
  label.value = account.label;
  baseUrl.value = account.baseUrl;
  token.value = "";
  dialogOpen.value = true;
}

async function save() {
  if (saving.value) return;
  saving.value = true;
  try {
    if (editing.value) {
      await updateGitAccount({
        id: editing.value.id,
        label: label.value.trim(),
        baseUrl: baseUrl.value.trim(),
        token: token.value,
      });
      toast.success(t("settings.accounts.updated"));
    } else {
      await addGitAccount({
        provider: provider.value,
        label: label.value.trim(),
        baseUrl: baseUrl.value.trim(),
        token: token.value,
      });
      toast.success(t("settings.accounts.added"));
    }
    dialogOpen.value = false;
    await reload();
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}

/** 待确认删除的账号,ConfirmDialog 确认后执行 */
const pendingRemove = ref<GitAccount | null>(null);
const removeConfirmOpen = computed({
  get: () => pendingRemove.value !== null,
  set: (v) => {
    if (!v) pendingRemove.value = null;
  },
});

const pendingRemoveName = computed(() => {
  const a = pendingRemove.value;
  return a ? a.label || a.username || providerLabel(a.provider) : "";
});

function remove(account: GitAccount) {
  pendingRemove.value = account;
}

async function confirmRemove() {
  const account = pendingRemove.value;
  if (!account) return;
  try {
    await removeGitAccount(account.id);
    toast.success(t("settings.accounts.deleted"));
    await reload();
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.accounts.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.accounts.description") }}</p>

    <div class="mt-4 flex items-center justify-between rounded-lg border px-3 py-2.5">
      <div class="flex flex-col gap-0.5">
        <span class="text-sm font-medium">{{ t("settings.accounts.enableGhCli") }}</span>
        <span class="text-xs text-muted-foreground">
          {{ t("settings.accounts.enableGhCliHint") }}
        </span>
      </div>
      <Switch v-model="enableGhCli" />
    </div>

    <div class="mt-4 flex flex-col gap-2">
      <div
        v-for="account in accounts"
        :key="account.id"
        class="flex items-center gap-3 rounded-md border px-3 py-2.5"
      >
        <GitBranch class="h-4 w-4 shrink-0 text-muted-foreground" />
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-2">
            <Badge variant="secondary" class="text-xs">{{ providerLabel(account.provider) }}</Badge>
            <span class="truncate text-sm font-medium">
              {{ account.label || account.username || providerLabel(account.provider) }}
            </span>
            <Badge
              v-if="account.tokenInvalid"
              variant="destructive"
              class="shrink-0 text-xs"
              :title="t('settings.accounts.tokenInvalidHint')"
            >
              {{ t("settings.accounts.tokenInvalid") }}
            </Badge>
          </div>
          <p class="mt-0.5 truncate text-xs text-muted-foreground">
            <template v-if="account.username">@{{ account.username }} · </template>
            <template v-if="account.provider === 'gitlab'">{{ account.baseUrl }} · </template>
            {{ account.tokenPreview }}
          </p>
        </div>
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8 shrink-0"
          :title="t('common.edit')"
          @click="openEdit(account)"
        >
          <Pencil class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8 shrink-0 text-destructive"
          :title="t('common.delete')"
          @click="remove(account)"
        >
          <Trash2 class="h-3.5 w-3.5" />
        </Button>
      </div>

      <p v-if="!loading && accounts.length === 0" class="text-sm text-muted-foreground">
        {{ t("settings.accounts.empty") }}
      </p>

      <div>
        <Button size="sm" variant="outline" class="gap-1.5" @click="openAdd">
          <Plus class="h-3.5 w-3.5" />
          {{ t("settings.accounts.add") }}
        </Button>
      </div>
    </div>

    <Dialog v-model:open="dialogOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {{ editing ? t("settings.accounts.editTitle") : t("settings.accounts.addTitle") }}
          </DialogTitle>
          <DialogDescription>{{ t("settings.accounts.formDescription") }}</DialogDescription>
        </DialogHeader>

        <form class="flex flex-col gap-4" @submit.prevent="save">
          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("settings.accounts.provider") }}</label>
            <div class="flex gap-1 rounded-md border p-1">
              <Button
                v-for="p in PROVIDERS"
                :key="p.value"
                type="button"
                variant="ghost"
                size="sm"
                class="h-7 flex-1"
                :class="provider === p.value && 'bg-accent'"
                :disabled="!!editing"
                @click="provider = p.value"
              >
                {{ p.label }}
              </Button>
            </div>
          </div>

          <div v-if="provider === 'gitlab'" class="flex flex-col gap-1.5">
            <label class="text-sm font-medium" for="account-base-url">
              {{ t("settings.accounts.baseUrl") }}
            </label>
            <Input
              id="account-base-url"
              v-model="baseUrl"
              :placeholder="t('settings.accounts.baseUrlPlaceholder')"
              spellcheck="false"
            />
          </div>

          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium" for="account-label">
              {{ t("settings.accounts.label") }}
            </label>
            <Input
              id="account-label"
              v-model="label"
              :placeholder="t('settings.accounts.labelPlaceholder')"
            />
          </div>

          <div class="flex flex-col gap-1.5">
            <label class="text-sm font-medium" for="account-token">
              {{ t("settings.accounts.token") }}
            </label>
            <Input
              id="account-token"
              v-model="token"
              type="password"
              :placeholder="
                editing
                  ? t('settings.accounts.tokenKeepPlaceholder')
                  : t('settings.accounts.tokenPlaceholder')
              "
              autocomplete="off"
              spellcheck="false"
            />
            <p class="text-xs text-muted-foreground">
              {{ t(`settings.accounts.tokenHint.${provider}`) }}
            </p>
          </div>

          <DialogFooter>
            <Button
              type="submit"
              :disabled="
                saving || (!editing && !token.trim()) || (provider === 'gitlab' && !baseUrl.trim())
              "
            >
              <Loader2 v-if="saving" class="h-4 w-4 animate-spin" />
              {{ saving ? t("settings.accounts.verifying") : t("common.save") }}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
    <ConfirmDialog
      v-model:open="removeConfirmOpen"
      :title="t('common.delete')"
      :description="t('settings.accounts.deleteConfirm', { name: pendingRemoveName })"
      :confirm-text="t('common.delete')"
      destructive
      @confirm="confirmRemove"
    />
  </section>
</template>
