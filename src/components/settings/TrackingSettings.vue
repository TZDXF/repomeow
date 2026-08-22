<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Search } from "@lucide/vue";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import type { Project } from "@/types";

const { t } = useI18n();
const store = useProjectsStore();
const settings = useSettingsStore();

// 设置页可能直达(未经项目列表页),项目列表为空时补拉一次(不带 git 状态)
onMounted(() => {
  if (!store.projects.length) {
    store.fetchProjects({ withGit: false });
  }
});

/** 搜索关键字,匹配名称/描述/路径 */
const searchInput = ref("");

const filteredProjects = computed(() => {
  // 空格切分为多个查询词,词间 AND:每个词至少命中名称/描述/路径之一
  const terms = searchInput.value.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return store.projects;
  return store.projects.filter((p) => {
    const fields = [p.name, p.description, p.path].map((s) => s.toLowerCase());
    return terms.every((q) => fields.some((f) => f.includes(q)));
  });
});

/** 已跟踪的项目排在前面,组内保持列表原有顺序 */
const sortedProjects = computed(() =>
  [...filteredProjects.value].sort((a, b) => Number(b.auto_pull) - Number(a.auto_pull)),
);

const trackedCount = computed(() => store.projects.filter((p) => p.auto_pull).length);

/** 逐项目切换中的开关置灰,避免连点造成命令乱序 */
const pendingIds = ref<Set<number>>(new Set());

async function toggle(project: Project, enabled: boolean) {
  if (pendingIds.value.has(project.id)) return;
  pendingIds.value.add(project.id);
  try {
    await store.setAutoPull(project.id, enabled);
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  } finally {
    pendingIds.value.delete(project.id);
  }
}

// ── Wiki 自动增量更新(跟踪联动) ──────────────────────────────────────────

/** 阈值输入的本地镜像:敲入非法值不打断,失焦/回车时收敛回 store 的合法值 */
const thresholdInput = ref(String(settings.wikiAutoUpdateThreshold));
watch(
  () => settings.wikiAutoUpdateThreshold,
  (v) => {
    thresholdInput.value = String(v);
  },
);

async function toggleWikiAutoUpdate(enabled: boolean) {
  try {
    await settings.setWikiAutoUpdate(enabled);
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  }
}

function commitThreshold() {
  const n = Number.parseInt(thresholdInput.value, 10);
  if (Number.isFinite(n)) {
    settings.setWikiAutoUpdateThreshold(n).catch((e) => {
      toast.error(e instanceof Error ? e.message : String(e));
    });
  } else {
    thresholdInput.value = String(settings.wikiAutoUpdateThreshold);
  }
}
</script>

<template>
  <section class="flex h-full flex-col">
    <h2 class="text-base font-semibold">{{ t("settings.tracking.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.tracking.description") }}
    </p>

    <!-- Wiki 自动增量更新(跟踪联动) -->
    <div class="mt-4 flex items-center justify-between gap-3 rounded-md border px-3 py-2.5">
      <div class="min-w-0 flex-1">
        <p class="text-sm font-medium">{{ t("settings.tracking.wikiAutoUpdateLabel") }}</p>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {{ t("settings.tracking.wikiAutoUpdateHint") }}
        </p>
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <div
          v-if="settings.wikiAutoUpdate"
          class="flex items-center gap-1.5"
          :title="t('settings.tracking.wikiThresholdTitle')"
        >
          <Input
            v-model="thresholdInput"
            type="number"
            min="1"
            max="10000"
            class="h-8 w-20 text-sm"
            @blur="commitThreshold"
            @keydown.enter.prevent="commitThreshold"
          />
          <span class="whitespace-nowrap text-xs text-muted-foreground">
            {{ t("settings.tracking.wikiThresholdSuffix") }}
          </span>
        </div>
        <Switch
          :model-value="settings.wikiAutoUpdate"
          :title="t('settings.tracking.wikiAutoUpdateLabel')"
          @update:model-value="toggleWikiAutoUpdate"
        />
      </div>
    </div>

    <div class="relative mt-4 w-64 max-w-full">
      <Search
        class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
      />
      <Input
        v-model="searchInput"
        :placeholder="t('settings.tracking.searchPlaceholder')"
        class="h-8 pl-8 text-sm"
      />
    </div>

    <p v-if="trackedCount" class="mt-3 text-xs text-muted-foreground">
      {{ t("settings.tracking.trackedCount", { count: trackedCount }) }}
    </p>

    <!-- 列表区跟随窗口高度:占满剩余空间,内部滚动 -->
    <ScrollArea class="mt-3 min-h-0 flex-1">
      <div class="flex flex-col gap-1">
        <div
          v-for="p in sortedProjects"
          :key="p.id"
          class="flex items-center justify-between gap-3 rounded-md px-2 py-1.5 hover:bg-accent"
        >
          <div class="min-w-0 flex-1">
            <p class="truncate text-sm font-medium">{{ p.name }}</p>
            <p class="truncate text-xs text-muted-foreground" :title="p.path">{{ p.path }}</p>
          </div>
          <Switch
            :model-value="p.auto_pull"
            :disabled="pendingIds.has(p.id)"
            :title="t('settings.tracking.toggleHint')"
            @update:model-value="toggle(p, $event)"
          />
        </div>
        <p v-if="!sortedProjects.length" class="py-6 text-center text-xs text-muted-foreground">
          {{
            store.projects.length ? t("settings.tracking.noMatch") : t("settings.tracking.empty")
          }}
        </p>
      </div>
    </ScrollArea>
  </section>
</template>
