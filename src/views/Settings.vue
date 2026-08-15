<script setup lang="ts">
import { computed, ref, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import {
  Archive,
  ArrowLeft,
  CalendarClock,
  Coffee,
  Info,
  KeyRound,
  MessageSquareText,
  SlidersHorizontal,
  Sparkles,
  Tags,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import GeneralSettings from "@/components/settings/GeneralSettings.vue";
import DevEnvSettings from "@/components/settings/DevEnvSettings.vue";
import TagSettings from "@/components/settings/TagSettings.vue";
import ArchiveSettings from "@/components/settings/ArchiveSettings.vue";
import AiSettings from "@/components/settings/AiSettings.vue";
import AccountSettings from "@/components/settings/AccountSettings.vue";
import PromptSettings from "@/components/settings/PromptSettings.vue";
import ReportScheduleSettings from "@/components/settings/ReportScheduleSettings.vue";
import AboutSettings from "@/components/settings/AboutSettings.vue";

const { t } = useI18n();
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
    id: "devEnv",
    labelKey: "settings.categories.devEnv",
    icon: Coffee,
    component: DevEnvSettings,
  },
  {
    id: "archive",
    labelKey: "settings.categories.archive",
    icon: Archive,
    component: ArchiveSettings,
  },
  { id: "ai", labelKey: "settings.categories.ai", icon: Sparkles, component: AiSettings },
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
    id: "schedule",
    labelKey: "settings.categories.schedule",
    icon: CalendarClock,
    component: ReportScheduleSettings,
  },
  { id: "about", labelKey: "settings.categories.about", icon: Info, component: AboutSettings },
];

const router = useRouter();
const activeId = ref(categories[0].id);
const active = computed(() => categories.find((c) => c.id === activeId.value) ?? categories[0]);
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
        <div class="max-w-xl p-6">
          <component :is="active.component" />
        </div>
      </ScrollArea>
    </div>
  </div>
</template>
