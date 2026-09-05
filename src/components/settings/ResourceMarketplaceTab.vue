<script setup lang="ts">
import { computed, onActivated, onDeactivated, ref } from "vue";
import { useI18n } from "vue-i18n";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "vue-sonner";
import {
  Check,
  ChevronDown,
  Download,
  ExternalLink,
  FolderOpen,
  RotateCw,
  Search,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  filterMarketplaceSkills,
  installResourceMarketplaceSkill,
  listResourceMarketplaceSkills,
  markMarketplaceInstalled,
  mergeMarketplaceSources,
  openResourceSkillDir,
  type ResourceMarketplaceSkill,
  type ResourceMarketplaceSource,
} from "@/lib/resource-library";

const { t } = useI18n();

const loading = ref(true);
/** 是否完成过至少一次加载:之后的后台刷新不清空列表,避免切回本页时闪烁 */
const loaded = ref(false);
const skills = ref<ResourceMarketplaceSkill[]>([]);
const sources = ref<ResourceMarketplaceSource[]>([]);
const query = ref("");
const submittedQuery = ref("");
const browseMode = ref<"all" | "trending" | "hot">("all");
const activeSourceId = ref<string | null>(null);
const sourcePickerOpen = ref(false);
const sourcePickerQuery = ref("");
const MAX_VISIBLE_SOURCES = 4;
type MarketplaceSnapshot = {
  skills: ResourceMarketplaceSkill[];
  sources: ResourceMarketplaceSource[];
};
/** 无关键词的榜单按模式缓存；搜索结果仍每次向市场查询。 */
const browseCache = new Map<"all" | "trending" | "hot", MarketplaceSnapshot>();
/** 正在安装的市场条目 id;非空表示有安装请求在途(串行,避免后端并发写入) */
const installingId = ref<string | null>(null);

const filtered = computed(() => filterMarketplaceSkills(skills.value, "", activeSourceId.value));
const sourceNameMap = computed(
  () => new Map(sources.value.map((source) => [source.id, source.name])),
);
const visibleSources = computed(() => sources.value.slice(0, MAX_VISIBLE_SOURCES));
const hiddenSources = computed(() => sources.value.slice(MAX_VISIBLE_SOURCES));
const searchedHiddenSources = computed(() => {
  const normalized = sourcePickerQuery.value.trim().toLowerCase();
  if (!normalized) return hiddenSources.value;
  return hiddenSources.value.filter((source) => source.name.toLowerCase().includes(normalized));
});

function sourceName(sourceId?: string): string {
  if (!sourceId) {
    return "";
  }
  return sourceNameMap.value.get(sourceId) ?? sourceId;
}

function formatInstalls(installs: number): string {
  if (installs >= 1_000_000) {
    return `${(installs / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
  }
  if (installs >= 1_000) {
    return `${(installs / 1_000).toFixed(1).replace(/\.0$/, "")}K`;
  }
  return String(installs);
}

let requestSeq = 0;

/** 每次加载先清空旧结果；响应只在它仍是当前请求、模式与搜索词未变时生效。 */
async function fetchList(force = false) {
  const requestId = ++requestSeq;
  const mode = browseMode.value;
  const searchQuery = submittedQuery.value;
  const cached = !searchQuery && !force ? browseCache.get(mode) : undefined;
  activeSourceId.value = null;
  if (cached) {
    skills.value = cached.skills;
    sources.value = cached.sources;
    loading.value = false;
    loaded.value = true;
    return;
  }
  loading.value = true;
  skills.value = [];
  sources.value = [];
  try {
    const result = await listResourceMarketplaceSkills({
      mode,
      ...(searchQuery ? { query: searchQuery } : {}),
    });
    if (
      requestId !== requestSeq ||
      mode !== browseMode.value ||
      searchQuery !== submittedQuery.value
    ) {
      return;
    }
    sources.value = mergeMarketplaceSources([], result.sources);
    skills.value = result.skills;
    if (!searchQuery) {
      browseCache.set(mode, { skills: result.skills, sources: sources.value });
    }
  } catch (e) {
    if (requestId === requestSeq) {
      toast.error(String(e));
    }
  } finally {
    if (requestId === requestSeq) {
      loading.value = false;
      loaded.value = true;
    }
  }
}

function switchMode(mode: "all" | "trending" | "hot") {
  if (mode === browseMode.value && loaded.value) {
    return;
  }
  browseMode.value = mode;
  query.value = "";
  submittedQuery.value = "";
  void fetchList();
}

function submitSearch() {
  submittedQuery.value = query.value.trim();
  void fetchList();
}

// 本页固定在 KeepAlive 内:onActivated 首次挂载与每次切回都会触发,顺带刷新已安装状态
onActivated(() => void fetchList());
onDeactivated(() => {
  requestSeq += 1;
});

function selectSource(id: string | null) {
  activeSourceId.value = id;
  sourcePickerOpen.value = false;
  sourcePickerQuery.value = "";
}

async function install(skill: ResourceMarketplaceSkill) {
  if (installingId.value !== null || skill.installedSkillId) {
    return;
  }
  installingId.value = skill.id;
  try {
    const created = await installResourceMarketplaceSkill(skill.id);
    skills.value = markMarketplaceInstalled(skills.value, skill.id, created.id);
    toast.success(
      t("settings.resources.market.installedToast", { name: created.name || skill.name }),
    );
  } catch (e) {
    toast.error(t("settings.resources.market.installFailed", { error: String(e) }));
  } finally {
    installingId.value = null;
  }
}

async function openSkillPage(skill: ResourceMarketplaceSkill) {
  try {
    await openUrl(skill.url);
  } catch (e) {
    toast.error(String(e));
  }
}

async function openInstalledDir(skill: ResourceMarketplaceSkill) {
  if (!skill.installedSkillId) {
    return;
  }
  try {
    await openResourceSkillDir(skill.installedSkillId);
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <section>
    <p class="text-sm text-muted-foreground">
      {{ t("settings.resources.market.description") }}
    </p>

    <div class="mt-3 flex flex-wrap items-center gap-2">
      <div class="flex rounded-md border p-0.5 text-xs">
        <button
          v-for="mode in ['all', 'trending', 'hot'] as const"
          :key="mode"
          type="button"
          class="rounded px-2 py-1 transition-colors"
          :class="browseMode === mode ? 'bg-accent text-foreground' : 'text-muted-foreground'"
          @click="switchMode(mode)"
        >
          {{ t(`settings.resources.market.modes.${mode}`) }}
        </button>
      </div>
      <div class="relative max-w-xs flex-1">
        <Search
          class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="query"
          class="h-8 pl-8 text-xs"
          :placeholder="t('settings.resources.market.searchPlaceholder')"
          spellcheck="false"
          @keydown.enter="submitSearch"
        />
      </div>
      <Button size="sm" class="h-8 shrink-0 gap-1.5" :disabled="loading" @click="submitSearch">
        <Search class="h-3.5 w-3.5" />
        {{ t("settings.resources.market.search") }}
      </Button>
      <Button
        variant="outline"
        size="sm"
        class="h-8 shrink-0"
        :disabled="loading"
        :title="t('settings.resources.market.refresh')"
        @click="fetchList(true)"
      >
        <RotateCw class="h-3.5 w-3.5" />
      </Button>
    </div>

    <div
      v-if="!loading && sources.length"
      class="mt-3 flex min-w-0 items-center gap-1.5 overflow-hidden"
    >
      <button
        type="button"
        class="shrink-0 rounded-full border px-2.5 py-1 text-xs transition-colors"
        :class="
          activeSourceId === null
            ? 'border-foreground bg-foreground text-background'
            : 'text-muted-foreground hover:bg-accent hover:text-foreground'
        "
        @click="selectSource(null)"
      >
        {{ t("settings.resources.market.allSources") }}
      </button>
      <button
        v-for="source in visibleSources"
        :key="source.id"
        type="button"
        class="max-w-40 shrink-0 truncate rounded-full border px-2.5 py-1 text-xs transition-colors"
        :title="source.url || source.name"
        :class="
          activeSourceId === source.id
            ? 'border-foreground bg-foreground text-background'
            : 'text-muted-foreground hover:bg-accent hover:text-foreground'
        "
        @click="selectSource(source.id)"
      >
        {{ source.name }}
      </button>
      <Popover v-if="hiddenSources.length" v-model:open="sourcePickerOpen">
        <PopoverTrigger as-child>
          <Button variant="outline" size="sm" class="h-7 shrink-0 gap-1 rounded-full px-2 text-xs">
            {{ t("settings.resources.market.moreSources", { count: hiddenSources.length }) }}
            <ChevronDown class="h-3 w-3" />
          </Button>
        </PopoverTrigger>
        <PopoverContent class="w-72 p-2" align="start" @open-auto-focus.prevent>
          <div class="relative">
            <Search
              class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              v-model="sourcePickerQuery"
              class="h-8 pl-8 text-xs"
              :placeholder="t('settings.resources.market.searchSourcesPlaceholder')"
              spellcheck="false"
            />
          </div>
          <div class="mt-1 max-h-56 overflow-y-auto">
            <button
              v-for="source in searchedHiddenSources"
              :key="source.id"
              type="button"
              class="hover:bg-accent w-full truncate rounded-sm px-2 py-1.5 text-left text-xs"
              :class="activeSourceId === source.id ? 'bg-accent text-foreground' : ''"
              :title="source.url || source.name"
              @click="selectSource(source.id)"
            >
              {{ source.name }}
            </button>
            <p v-if="!searchedHiddenSources.length" class="px-2 py-2 text-xs text-muted-foreground">
              {{ t("settings.resources.market.noMatchingSources") }}
            </p>
          </div>
        </PopoverContent>
      </Popover>
    </div>

    <p v-if="loading" class="mt-6 text-center text-xs text-muted-foreground">
      {{ t("common.loading") }}
    </p>
    <template v-else-if="filtered.length">
      <div class="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-2">
        <div v-for="skill in filtered" :key="skill.id" class="group rounded-lg border p-3">
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <p class="truncate text-sm font-medium" :title="skill.name">{{ skill.name }}</p>
              <p class="mt-0.5 truncate text-[11px] text-muted-foreground">
                {{ skill.source }}
              </p>
            </div>
            <span class="flex shrink-0 items-center gap-1">
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                :title="t('settings.resources.market.openPage')"
                @click="openSkillPage(skill)"
              >
                <ExternalLink class="h-3.5 w-3.5" />
              </Button>
              <template v-if="skill.installedSkillId">
                <span
                  class="flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground"
                >
                  <Check class="h-3 w-3 text-emerald-600 dark:text-emerald-400" />
                  {{ t("settings.resources.market.installed") }}
                </span>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="t('settings.resources.market.openDir')"
                  @click="openInstalledDir(skill)"
                >
                  <FolderOpen class="h-3.5 w-3.5" />
                </Button>
              </template>
              <Button
                v-else
                size="sm"
                class="h-7 gap-1 px-2 text-xs"
                :disabled="installingId !== null"
                @click="install(skill)"
              >
                <Download
                  class="h-3.5 w-3.5"
                  :class="{ 'animate-pulse': installingId === skill.id }"
                />
                {{
                  installingId === skill.id
                    ? t("settings.resources.market.installing")
                    : t("settings.resources.market.install")
                }}
              </Button>
            </span>
          </div>
          <p v-if="skill.description" class="mt-1 line-clamp-2 text-xs text-muted-foreground">
            {{ skill.description }}
          </p>
          <div class="mt-2 flex items-center gap-2">
            <Badge
              v-if="sourceName(skill.source)"
              variant="outline"
              class="max-w-40 truncate text-[10px]"
            >
              {{ sourceName(skill.source) }}
            </Badge>
            <span
              class="flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground"
              :title="t('settings.resources.market.installCount', { count: skill.installs })"
            >
              <Download class="h-3 w-3" />
              {{ formatInstalls(skill.installs) }}
            </span>
          </div>
        </div>
      </div>
    </template>

    <p
      v-else-if="!loading"
      class="mt-6 rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
    >
      {{
        submittedQuery || activeSourceId
          ? t("settings.resources.market.noMatch")
          : t("settings.resources.market.empty")
      }}
    </p>
  </section>
</template>
