<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Search } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { matchesTrackingProject } from "@/lib/tracking";
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

/** 搜索关键字,匹配名称/描述/路径/标签 */
const searchInput = ref("");

const filteredProjects = computed(() => {
  return store.projects.filter((project) => matchesTrackingProject(project, searchInput.value));
});

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

/** Wiki 自动更新开关的切换中状态(与跟踪开关分开置灰,互不阻塞) */
const wikiPendingIds = ref<Set<number>>(new Set());

async function toggleWiki(project: Project, enabled: boolean) {
  if (wikiPendingIds.value.has(project.id)) return;
  wikiPendingIds.value.add(project.id);
  try {
    await store.setWikiAutoUpdate(project.id, enabled);
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  } finally {
    wikiPendingIds.value.delete(project.id);
  }
}

/** Wiki 开关提示:全局已开启时项目值被忽略,否则可独立配置。 */
function wikiToggleTitle(): string {
  if (settings.wikiAutoUpdate) return t("settings.tracking.wikiToggleGloballyOn");
  return t("settings.tracking.wikiToggleHint");
}

// ── Wiki 自动增量更新(跟踪联动) ──────────────────────────────────────────

async function toggleWikiAutoUpdate(enabled: boolean) {
  try {
    await settings.setWikiAutoUpdate(enabled);
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
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
      <Switch
        class="shrink-0"
        :model-value="settings.wikiAutoUpdate"
        :title="t('settings.tracking.wikiAutoUpdateLabel')"
        @update:model-value="toggleWikiAutoUpdate"
      />
    </div>

    <div class="relative mt-4 w-80 max-w-full">
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

    <!-- 表格区跟随窗口高度:切换开关仅更新当前行,不改变后端名称排序 -->
    <ScrollArea class="mt-3 min-h-0 flex-1 rounded-md border">
      <table class="w-full table-fixed text-sm">
        <thead class="sticky top-0 z-10 bg-background">
          <tr class="border-b text-left text-xs text-muted-foreground">
            <th class="px-3 py-2 font-medium">
              {{ t("settings.tracking.columns.project") }}
            </th>
            <th class="w-24 px-3 py-2 text-center font-medium">
              {{ t("settings.tracking.columns.wiki") }}
            </th>
            <th class="w-24 px-3 py-2 text-center font-medium">
              {{ t("settings.tracking.columns.tracking") }}
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="p in filteredProjects"
            :key="p.id"
            class="border-b transition-colors last:border-0 hover:bg-accent/60"
          >
            <td class="px-3 py-2">
              <p class="truncate font-medium" :title="p.name">{{ p.name }}</p>
              <div v-if="p.tags.length" class="mt-1.5 flex flex-wrap gap-1">
                <Badge
                  v-for="tag in p.tags"
                  :key="tag.id"
                  variant="secondary"
                  class="px-1.5 py-0 text-[11px]"
                  :style="{ backgroundColor: tag.color + '22', color: tag.color }"
                >
                  {{ tag.name }}
                </Badge>
              </div>
              <p
                class="mt-0.5 truncate text-xs text-muted-foreground"
                :title="p.description || undefined"
              >
                {{ p.description || "-" }}
              </p>
            </td>
            <td class="px-3 py-2">
              <div class="flex justify-center">
                <Switch
                  :model-value="p.wiki_auto_update"
                  :disabled="settings.wikiAutoUpdate || wikiPendingIds.has(p.id)"
                  :title="wikiToggleTitle()"
                  @update:model-value="toggleWiki(p, $event)"
                />
              </div>
            </td>
            <td class="px-3 py-2">
              <div class="flex justify-center">
                <Switch
                  :model-value="p.auto_pull"
                  :disabled="pendingIds.has(p.id)"
                  :title="t('settings.tracking.toggleHint')"
                  @update:model-value="toggle(p, $event)"
                />
              </div>
            </td>
          </tr>
          <tr v-if="!filteredProjects.length">
            <td colspan="3" class="py-8 text-center text-xs text-muted-foreground">
              {{
                store.projects.length
                  ? t("settings.tracking.noMatch")
                  : t("settings.tracking.empty")
              }}
            </td>
          </tr>
        </tbody>
      </table>
    </ScrollArea>
  </section>
</template>
