<script setup lang="ts">
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Check, FolderOpen, Plus, ScanSearch, Trash2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { cmd } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settings";
import type { JdkCandidate, JdkConfig } from "@/types";

const { t } = useI18n();
const store = useSettingsStore();

const detecting = ref(false);
const dialogOpen = ref(false);
const formPath = ref("");
const formName = ref("");
const formVersion = ref("");
const formError = ref("");
const nameTouched = ref(false);
const checking = ref(false);
const submitting = ref(false);

/** 从 `java -version` 版本串提取展示用主版本:"17.0.2" -> "17","1.8.0_392" -> "8" */
function majorVersion(version: string): string {
  const parts = version.split(/[._]/);
  if (parts[0] === "1" && parts.length > 1) return parts[1];
  return parts[0] || version;
}

/** 路径去重 key:Windows 路径大小写不敏感 */
function pathKey(path: string): string {
  return path.toLowerCase();
}

async function detect() {
  if (detecting.value) return;
  detecting.value = true;
  try {
    const candidates = await cmd<JdkCandidate[]>("detect_jdks");
    // 探测结果先按路径去重,入库去重(addJdks 内按大小写不敏感路径再过滤一次)
    const seen = new Set<string>();
    const jdks: JdkConfig[] = [];
    for (const c of candidates) {
      if (seen.has(pathKey(c.path))) continue;
      seen.add(pathKey(c.path));
      jdks.push({
        id: crypto.randomUUID(),
        name: `Java ${majorVersion(c.version)}`,
        path: c.path,
      });
    }
    const added = await store.addJdks(jdks);
    if (added) {
      toast.success(t("settings.devEnv.detected", { count: added }));
    } else {
      toast.info(t("settings.devEnv.noneDetected"));
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    detecting.value = false;
  }
}

function openCreate() {
  formPath.value = "";
  formName.value = "";
  formVersion.value = "";
  formError.value = "";
  nameTouched.value = false;
  dialogOpen.value = true;
}

async function pickFolder() {
  const selected = await openDialog({ directory: true, multiple: false });
  if (typeof selected === "string") {
    formPath.value = selected;
    await validatePath();
  }
}

/** 选定目录后立即校验并取版本,版本就绪且用户未改过名称时预填默认名 */
async function validatePath() {
  formVersion.value = "";
  formError.value = "";
  const path = formPath.value.trim();
  if (!path) return;
  checking.value = true;
  try {
    const version = await cmd<string>("check_jdk", { path });
    formVersion.value = version;
    if (!nameTouched.value) formName.value = `Java ${majorVersion(version)}`;
  } catch (e) {
    formError.value = String(e);
  } finally {
    checking.value = false;
  }
}

async function submit() {
  const path = formPath.value.trim();
  const name = formName.value.trim();
  if (!path || !name || !formVersion.value || submitting.value) return;
  if (store.jdkList.some((j) => pathKey(j.path) === pathKey(path))) {
    formError.value = t("settings.devEnv.duplicatePath");
    return;
  }
  submitting.value = true;
  try {
    await store.saveJdk({ id: crypto.randomUUID(), name, path });
    dialogOpen.value = false;
    toast.success(t("settings.devEnv.added"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}

async function remove(jdk: JdkConfig) {
  if (!window.confirm(t("settings.devEnv.deleteConfirm", { name: jdk.name }))) return;
  await store.removeJdk(jdk.id);
}
</script>

<template>
  <section>
    <div class="flex items-start justify-between gap-4">
      <h2 class="text-base font-semibold">{{ t("settings.devEnv.title") }}</h2>
      <div class="flex shrink-0 gap-2">
        <Button size="sm" variant="outline" :disabled="detecting" @click="detect">
          <ScanSearch class="h-4 w-4" />
          {{ detecting ? t("settings.devEnv.detecting") : t("settings.devEnv.detect") }}
        </Button>
        <Button size="sm" variant="outline" @click="openCreate">
          <Plus class="h-4 w-4" />
          {{ t("settings.devEnv.add") }}
        </Button>
      </div>
    </div>

    <div v-if="store.jdkList.length" class="mt-4 flex flex-col gap-2">
      <div
        v-for="jdk in store.jdkList"
        :key="jdk.id"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 transition-colors hover:bg-accent"
        :class="store.defaultJdkId === jdk.id && 'border-primary'"
      >
        <button
          type="button"
          class="flex min-w-0 flex-1 items-center gap-3 text-left"
          :title="t('settings.devEnv.setDefault')"
          @click="store.setDefaultJdk(jdk.id)"
        >
          <span class="min-w-0 flex-1">
            <span class="block truncate text-sm font-medium">{{ jdk.name }}</span>
            <span class="block truncate font-mono text-xs text-muted-foreground" :title="jdk.path">
              {{ jdk.path }}
            </span>
          </span>
          <Check v-if="store.defaultJdkId === jdk.id" class="h-4 w-4 shrink-0 text-primary" />
        </button>
        <Button size="icon-sm" variant="ghost" :title="t('common.delete')" @click="remove(jdk)">
          <Trash2 class="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
    <p v-else class="mt-4 text-sm text-muted-foreground">
      {{ t("settings.devEnv.empty") }}
    </p>
  </section>

  <Dialog v-model:open="dialogOpen">
    <DialogContent class="sm:max-w-[min(34rem,calc(100%-2rem))]">
      <DialogHeader>
        <DialogTitle>{{ t("settings.devEnv.dialogTitle") }}</DialogTitle>
      </DialogHeader>
      <form class="flex flex-col gap-3" @submit.prevent="submit">
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("settings.devEnv.pathLabel") }}</label>
          <div class="flex gap-2">
            <Input
              v-model="formPath"
              :placeholder="t('settings.devEnv.pathPlaceholder')"
              @change="validatePath"
            />
            <Button type="button" variant="outline" @click="pickFolder">
              <FolderOpen class="h-4 w-4" />
            </Button>
          </div>
          <p v-if="formVersion" class="text-xs text-primary">
            {{ t("settings.devEnv.versionOk", { version: formVersion }) }}
          </p>
          <p v-else-if="formError" class="text-xs text-destructive">{{ formError }}</p>
          <p v-else class="text-xs text-muted-foreground">
            {{ t("settings.devEnv.pathHint") }}
          </p>
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("settings.devEnv.nameLabel") }}</label>
          <Input
            v-model="formName"
            :placeholder="t('settings.devEnv.namePlaceholder')"
            @input="nameTouched = true"
          />
        </div>
        <DialogFooter>
          <Button
            type="submit"
            :disabled="!formPath.trim() || !formName.trim() || !formVersion || submitting"
          >
            {{ submitting ? t("common.saving") : t("common.save") }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
