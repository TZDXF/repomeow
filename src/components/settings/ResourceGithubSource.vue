<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "vue-sonner";
import { Check, Download, ExternalLink, Search } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import {
  listResourceMarketplaceSkills,
  installResourceMarketplaceSkill,
  markMarketplaceInstalled,
  type ResourceMarketplaceSkill,
} from "@/lib/resource-library";

const props = defineProps<{ initialSource?: string }>();
const { t } = useI18n();
const sourceInput = ref(props.initialSource || "");
const loadedSource = ref("");
const query = ref("");
const skills = ref<ResourceMarketplaceSkill[]>([]);
const loading = ref(false);
const error = ref("");
const installing = ref(new Set<string>());
const batchRunning = ref(false);
const confirmOpen = ref(false);
const progress = ref({ completed: 0, total: 0, success: 0, failed: 0 });
const failures = ref<Record<string, string>>({});
const cancelRequested = ref(false);
let sequence = 0;
let disposed = false;
const filtered = computed(() =>
  skills.value.filter((skill) =>
    `${skill.name} ${skill.id}`.toLowerCase().includes(query.value.trim().toLowerCase()),
  ),
);
const pending = computed(() => skills.value.filter((skill) => !skill.installedSkillId));
async function load() {
  if (batchRunning.value || installing.value.size) {
    return;
  }
  const source = sourceInput.value.trim();
  if (!source) {
    return;
  }
  const seq = ++sequence;
  loading.value = true;
  error.value = "";
  skills.value = [];
  loadedSource.value = "";
  query.value = "";
  failures.value = {};
  progress.value = { completed: 0, total: 0, success: 0, failed: 0 };
  try {
    const result = await listResourceMarketplaceSkills({ mode: "all", source, refresh: true });
    if (seq !== sequence) {
      return;
    }
    skills.value = result.skills;
    loadedSource.value = result.sources[0]?.id || source;
  } catch (e) {
    if (seq === sequence) {
      error.value = String(e);
    }
  } finally {
    if (seq === sequence) {
      loading.value = false;
    }
  }
}
async function install(skill: ResourceMarketplaceSkill): Promise<boolean> {
  if (skill.installedSkillId || installing.value.has(skill.id)) {
    return false;
  }
  installing.value.add(skill.id);
  delete failures.value[skill.id];
  try {
    const created = await installResourceMarketplaceSkill(skill.id);
    if (!disposed) {
      skills.value = markMarketplaceInstalled(skills.value, skill.id, created.id);
    }
    return true;
  } catch (e) {
    if (!disposed) {
      failures.value[skill.id] = String(e);
    }
    return false;
  } finally {
    installing.value.delete(skill.id);
  }
}
async function installAll() {
  if (batchRunning.value || installing.value.size || loading.value) {
    return;
  }
  confirmOpen.value = false;
  // 来源全量列表，不使用界面关键词过滤结果；失败项仍为未安装，下次可重试。
  const targets = [...pending.value];
  batchRunning.value = true;
  cancelRequested.value = false;
  progress.value = { completed: 0, total: targets.length, success: 0, failed: 0 };
  try {
    for (const skill of targets) {
      if (cancelRequested.value || disposed) {
        break;
      }
      const ok = await install(skill);
      progress.value.completed++;
      if (ok) {
        progress.value.success++;
      } else {
        progress.value.failed++;
      }
    }
  } finally {
    batchRunning.value = false;
  }
}
async function openSkill(skill: ResourceMarketplaceSkill) {
  try {
    await openUrl(skill.url);
  } catch (e) {
    toast.error(String(e));
  }
}
watch(
  () => props.initialSource,
  (source) => {
    if (!source || batchRunning.value || installing.value.size) {
      return;
    }
    sourceInput.value = source;
    void load();
  },
  { immediate: true },
);
onBeforeUnmount(() => {
  disposed = true;
  sequence++;
  cancelRequested.value = true;
});
</script>

<template>
  <div class="mt-3 space-y-3">
    <p class="text-xs text-muted-foreground">
      {{ t("settings.resources.market.remote.sourceHint") }}
    </p>
    <form class="flex gap-2" @submit.prevent="load">
      <Input
        v-model="sourceInput"
        :disabled="batchRunning || installing.size > 0"
        :placeholder="t('settings.resources.market.remote.sourcePlaceholder')"
        spellcheck="false"
      />
      <Button
        type="submit"
        :disabled="loading || batchRunning || installing.size > 0 || !sourceInput.trim()"
      >
        <Search class="mr-1 h-4 w-4" />{{ t("settings.resources.market.remote.fetchSource") }}
      </Button>
    </form>
    <p v-if="loading" class="text-xs text-muted-foreground">{{ t("common.loading") }}</p>
    <p v-if="error" role="alert" class="text-xs text-destructive">{{ error }}</p>
    <template v-if="loadedSource">
      <div class="flex flex-wrap items-center gap-2">
        <span class="text-sm font-medium">{{ loadedSource }}</span>
        <span class="text-xs text-muted-foreground">{{
          t("settings.resources.market.remote.sourceCount", { count: skills.length })
        }}</span>
        <Button
          size="sm"
          :disabled="!pending.length || batchRunning || installing.size > 0"
          @click="confirmOpen = true"
        >
          <Download class="mr-1 h-3.5 w-3.5" />{{
            t("settings.resources.market.remote.installAll", { count: pending.length })
          }}
        </Button>
        <Button
          v-if="batchRunning"
          variant="outline"
          size="sm"
          :disabled="cancelRequested"
          @click="cancelRequested = true"
        >
          {{
            t(
              cancelRequested
                ? "settings.resources.market.remote.stopping"
                : "settings.resources.market.remote.stop",
            )
          }}
        </Button>
      </div>
      <p v-if="progress.total" role="status" class="text-xs text-muted-foreground">
        {{ t("settings.resources.market.remote.progress", progress) }}
      </p>
      <Input
        v-if="skills.length"
        v-model="query"
        :placeholder="t('settings.resources.market.remote.filterSource')"
      />
      <p v-if="!filtered.length" class="text-xs text-muted-foreground">
        {{ t("settings.resources.market.noMatch") }}
      </p>
      <div class="grid grid-cols-1 gap-3 lg:grid-cols-2">
        <div v-for="skill in filtered" :key="skill.id" class="min-w-0 rounded-lg border p-3">
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <p class="truncate text-sm font-medium" :title="skill.name">{{ skill.name }}</p>
              <p class="mt-1 break-all text-xs text-muted-foreground">
                {{ skill.id.replace(/^github:/, "") }}
              </p>
            </div>
            <div class="flex shrink-0 items-center gap-1">
              <Button
                variant="ghost"
                size="icon"
                :title="t('settings.resources.market.openPage')"
                @click="openSkill(skill)"
                ><ExternalLink class="h-3.5 w-3.5"
              /></Button>
              <span v-if="skill.installedSkillId" class="flex items-center gap-1 text-xs"
                ><Check class="h-3.5 w-3.5 text-emerald-600" />{{
                  t("settings.resources.market.installed")
                }}</span
              >
              <Button
                v-else
                size="sm"
                :disabled="batchRunning || installing.has(skill.id)"
                @click="install(skill)"
                >{{
                  t(
                    installing.has(skill.id)
                      ? "settings.resources.market.installing"
                      : "settings.resources.market.install",
                  )
                }}</Button
              >
            </div>
          </div>
          <p
            v-if="failures[skill.id]"
            role="alert"
            class="mt-2 break-words text-xs text-destructive"
          >
            {{ failures[skill.id] }}
          </p>
        </div>
      </div>
    </template>
    <Dialog v-model:open="confirmOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{{
            t("settings.resources.market.remote.installAll", { count: pending.length })
          }}</DialogTitle>
          <DialogDescription>{{
            t("settings.resources.market.remote.confirmInstall", {
              source: loadedSource,
              count: pending.length,
            })
          }}</DialogDescription>
        </DialogHeader>
        <DialogFooter
          ><Button @click="installAll">{{
            t("settings.resources.market.install")
          }}</Button></DialogFooter
        >
      </DialogContent>
    </Dialog>
  </div>
</template>
