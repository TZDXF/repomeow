<script setup lang="ts">
import { computed, nextTick, ref, watch, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import {
  Archive,
  ArrowLeft,
  Boxes,
  CalendarClock,
  Cable,
  Coffee,
  Gauge,
  Info,
  KeyRound,
  MessageSquareText,
  Radar,
  Search,
  SlidersHorizontal,
  Sparkles,
  Tags,
} from "@lucide/vue";
import { onClickOutside } from "@vueuse/core";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import ScrollArea from "@/components/common/ScrollArea.vue";
import GeneralSettings from "@/components/settings/GeneralSettings.vue";
import ToolchainPanel from "@/components/settings/ToolchainPanel.vue";
import CliSettings from "@/components/settings/CliSettings.vue";
import TagSettings from "@/components/settings/TagSettings.vue";
import TrackingSettings from "@/components/settings/TrackingSettings.vue";
import ArchiveSettings from "@/components/settings/ArchiveSettings.vue";
import AiSettings from "@/components/settings/AiSettings.vue";
import AiUsageSettings from "@/components/settings/AiUsageSettings.vue";
import AccountSettings from "@/components/settings/AccountSettings.vue";
import PromptSettings from "@/components/settings/PromptSettings.vue";
import ReportScheduleSettings from "@/components/settings/ReportScheduleSettings.vue";
import ResourceLibrarySettings from "@/components/settings/ResourceLibrarySettings.vue";
import AboutSettings from "@/components/settings/AboutSettings.vue";

import {
  resourceSettingsSearchEntries,
  resourceTabForSetting,
  matchesSettingsSearch,
  settingsSearchText,
} from "@/lib/settings-search";

const { t, tm } = useI18n();
interface Category {
  id: string;
  labelKey: string;
  icon: Component;
  component: Component;
}

const categories: Category[] = [
  {
    id: "general",
    labelKey: "settings.categories.general",
    icon: SlidersHorizontal,
    component: GeneralSettings,
  },
  { id: "tags", labelKey: "settings.categories.tags", icon: Tags, component: TagSettings },
  {
    id: "tracking",
    labelKey: "settings.categories.tracking",
    icon: Radar,
    component: TrackingSettings,
  },
  {
    id: "devEnv",
    labelKey: "settings.categories.devEnv",
    icon: Coffee,
    component: ToolchainPanel,
  },
  {
    id: "archive",
    labelKey: "settings.categories.archive",
    icon: Archive,
    component: ArchiveSettings,
  },
  { id: "cli", labelKey: "settings.categories.cli", icon: Cable, component: CliSettings },
  { id: "ai", labelKey: "settings.categories.ai", icon: Sparkles, component: AiSettings },
  {
    id: "aiUsage",
    labelKey: "settings.categories.aiUsage",
    icon: Gauge,
    component: AiUsageSettings,
  },
  {
    id: "accounts",
    labelKey: "settings.categories.accounts",
    icon: KeyRound,
    component: AccountSettings,
  },
  {
    id: "prompts",
    labelKey: "settings.categories.prompts",
    icon: MessageSquareText,
    component: PromptSettings,
  },
  {
    id: "resources",
    labelKey: "settings.categories.resources",
    icon: Boxes,
    component: ResourceLibrarySettings,
  },
  {
    id: "schedule",
    labelKey: "settings.categories.schedule",
    icon: CalendarClock,
    component: ReportScheduleSettings,
  },
  { id: "about", labelKey: "settings.categories.about", icon: Info, component: AboutSettings },
];

const router = useRouter();
const route = useRoute();
// 支持从子页(如技能预览页)带 ?category=resources 回跳,直接落在原分类
const initialCategory = String(route.query.category ?? "");
const activeId = ref(
  categories.some((c) => c.id === initialCategory) ? initialCategory : categories[0].id,
);
const active = computed(() => categories.find((c) => c.id === activeId.value) ?? categories[0]);

// 仅索引页面上可直接访问的设置；不挂载隐藏面板，避免触发额外请求。
const settingEntries = [
  { category: "general", labelKey: "settings.general.theme" },
  { category: "general", labelKey: "settings.skin.title" },
  { category: "general", labelKey: "settings.mdTheme.title" },
  { category: "general", labelKey: "settings.general.openWith" },
  { category: "general", labelKey: "settings.general.worktreeDir" },
  { category: "general", labelKey: "settings.tray.title" },
  { category: "general", labelKey: "settings.general.language" },
  { category: "general", labelKey: "settings.developer.title" },
  { category: "general", labelKey: "settings.autostart.launchAtLogin" },
  { category: "ai", labelKey: "settings.ai.title" },
  { category: "ai", labelKey: "settings.ai.defaultModel" },
  { category: "ai", labelKey: "settings.ai.providers" },
  { category: "ai", labelKey: "settings.ai.concurrency" },
  { category: "aiUsage", labelKey: "settings.usage.title" },
  { category: "aiUsage", labelKey: "settings.usage.trend" },
  { category: "aiUsage", labelKey: "settings.usage.distribution" },
  { category: "aiUsage", labelKey: "settings.usage.details" },
];
const categorySearchKeys: Record<string, string[]> = {
  general: [
    "settings.general",
    "settings.theme",
    "settings.skin",
    "settings.mdTheme",
    "settings.terminal",
    "settings.tray",
    "settings.language",
    "settings.autostart",
    "settings.developer",
  ],
  tags: ["settings.tags"],
  tracking: ["settings.tracking"],
  devEnv: ["settings.devEnv"],
  archive: ["settings.archive"],
  cli: ["settings.cli"],
  ai: ["settings.ai"],
  aiUsage: ["settings.usage"],
  accounts: ["settings.accounts"],
  prompts: ["settings.prompts"],
  resources: ["settings.resources"],
  schedule: ["reportSchedule"],
  about: ["settings.about", "settings.update", "update"],
};
const query = ref("");
const searchOpen = ref(false);
const selectedIndex = ref(0);
const searchRoot = ref<HTMLElement>();
const content = ref<HTMLElement>();
const resourceLibrary = ref<InstanceType<typeof ResourceLibrarySettings>>();
onClickOutside(searchRoot, () => {
  searchOpen.value = false;
});
const searchResults = computed(() => {
  if (!query.value.trim()) return [];
  const entries = [
    ...settingEntries,
    ...resourceSettingsSearchEntries,
    ...categories.map((c) => ({ category: c.id, labelKey: c.labelKey })),
  ];
  return entries.filter((entry) => {
    const category = categories.find((c) => c.id === entry.category)!;
    // 分类结果同时覆盖描述与子项，未直接展示的设置仍可通过分类入口访问。
    const resourceEntry = resourceSettingsSearchEntries.find(
      (item) => item.labelKey === entry.labelKey,
    );
    const details =
      entry.labelKey === category.labelKey
        ? (categorySearchKeys[entry.category] ?? [])
            .map((key) => settingsSearchText(tm(key)))
            .join(" ")
        : resourceEntry
          ? settingsSearchText(tm(resourceEntry.searchKey))
          : "";
    return matchesSettingsSearch(
      query.value,
      `${t(category.labelKey)} ${t(entry.labelKey)} ${entry.labelKey} ${details}`,
    );
  });
});
watch(query, () => {
  selectedIndex.value = 0;
  searchOpen.value = true;
});
function moveSelection(delta: number) {
  if (!searchResults.value.length) return;
  searchOpen.value = true;
  selectedIndex.value =
    (selectedIndex.value + delta + searchResults.value.length) % searchResults.value.length;
  nextTick(() =>
    searchRoot.value?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" }),
  );
}
async function locateSetting(entry = searchResults.value[selectedIndex.value]) {
  if (!entry) return;
  activeId.value = entry.category;
  searchOpen.value = false;
  await nextTick();
  const resourceTab = resourceTabForSetting(entry.labelKey);
  if (entry.category === "resources" && resourceTab) {
    resourceLibrary.value?.selectTab(resourceTab);
    await nextTick();
  }
  const target = content.value?.querySelector<HTMLElement>(`[data-setting="${entry.labelKey}"]`);
  if (target) {
    target.scrollIntoView({ block: "center", behavior: "smooth" });
    if (!target.hasAttribute("tabindex")) target.setAttribute("tabindex", "-1");
    target.focus({ preventScroll: true });
  } else {
    content.value?.closest('[data-slot="scroll-area-viewport"]')?.scrollTo({ top: 0 });
    content.value?.focus({ preventScroll: true });
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex shrink-0 items-center gap-2 border-b px-4 py-2.5">
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
        :title="t('settings.back')"
        @click="router.push('/')"
      >
        <ArrowLeft class="h-4 w-4" />
      </Button>
      <h1 class="text-sm font-semibold">{{ t("settings.title") }}</h1>
      <div
        ref="searchRoot"
        class="relative ml-auto w-72 max-w-[60%]"
        @keydown.esc="searchOpen = false"
      >
        <Search
          class="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground"
        />
        <Input
          v-model="query"
          class="h-9 pl-9"
          :placeholder="t('settings.search.placeholder')"
          :aria-label="t('settings.search.placeholder')"
          role="combobox"
          aria-autocomplete="list"
          aria-controls="settings-search-results"
          :aria-expanded="searchOpen && !!query.trim()"
          :aria-activedescendant="
            searchOpen && searchResults.length ? `settings-result-${selectedIndex}` : undefined
          "
          @focus="searchOpen = true"
          @keydown.down.prevent="moveSelection(1)"
          @keydown.up.prevent="moveSelection(-1)"
          @keydown.enter.prevent="locateSetting()"
        />
        <div
          v-if="searchOpen && query.trim()"
          id="settings-search-results"
          role="listbox"
          :aria-label="t('settings.search.placeholder')"
          class="absolute right-0 top-full z-50 mt-1 max-h-80 w-full overflow-y-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md"
        >
          <button
            v-for="(result, index) in searchResults"
            :id="`settings-result-${index}`"
            :key="result.labelKey"
            type="button"
            role="option"
            :aria-selected="selectedIndex === index"
            class="flex w-full flex-col rounded-sm px-3 py-2 text-left text-sm hover:bg-accent"
            :class="selectedIndex === index && 'bg-accent'"
            @click="locateSetting(result)"
          >
            <span>{{ t(result.labelKey) }}</span>
            <span class="text-xs text-muted-foreground">{{
              t(categories.find((c) => c.id === result.category)!.labelKey)
            }}</span>
          </button>
          <p v-if="!searchResults.length" class="px-3 py-4 text-sm text-muted-foreground">
            {{ t("settings.search.empty") }}
          </p>
        </div>
      </div>
    </header>

    <div class="flex flex-1 overflow-hidden">
      <nav class="w-44 shrink-0 border-r p-2">
        <button
          v-for="c in categories"
          :key="c.id"
          type="button"
          class="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          :class="activeId === c.id && 'bg-accent font-medium text-foreground'"
          @click="activeId = c.id"
        >
          <component :is="c.icon" class="h-3.5 w-3.5" />
          {{ t(c.labelKey) }}
        </button>
      </nav>

      <ScrollArea class="flex-1">
        <!-- h-full + flex-col:内容矮于窗口时撑满,供列表型设置页(跟踪/归档)的列表区 flex-1 跟随窗口高度;
             资源管理为宽布局分类,内容区放宽 -->
        <div
          ref="content"
          tabindex="-1"
          class="settings-content flex h-full flex-col p-6 outline-none"
          :class="activeId === 'resources' ? 'max-w-4xl' : 'max-w-xl'"
        >
          <ResourceLibrarySettings v-if="activeId === 'resources'" ref="resourceLibrary" />
          <component v-else :is="active.component" />
        </div>
      </ScrollArea>
    </div>
  </div>
</template>

<style scoped>
.settings-content :deep([data-setting]:focus) {
  outline: 2px solid var(--primary);
  outline-offset: 6px;
  border-radius: 2px;
}
</style>
