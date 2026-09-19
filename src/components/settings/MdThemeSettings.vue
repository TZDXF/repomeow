<script setup lang="ts">
import { useI18n } from "vue-i18n";
import type { Component } from "vue";
import { Blend, BookOpenText, Check, FileText, NotebookText } from "@lucide/vue";
import { useSettingsStore, type MdTheme } from "@/stores/settings";

const { t } = useI18n();
const store = useSettingsStore();

const OPTIONS: { value: MdTheme; labelKey: string; descriptionKey: string; icon: Component }[] = [
  {
    value: "default",
    labelKey: "settings.mdTheme.default",
    descriptionKey: "settings.mdTheme.defaultDesc",
    icon: Blend,
  },
  {
    value: "github",
    labelKey: "settings.mdTheme.github",
    descriptionKey: "settings.mdTheme.githubDesc",
    icon: FileText,
  },
  {
    value: "notion",
    labelKey: "settings.mdTheme.notion",
    descriptionKey: "settings.mdTheme.notionDesc",
    icon: NotebookText,
  },
  {
    value: "serif",
    labelKey: "settings.mdTheme.serif",
    descriptionKey: "settings.mdTheme.serifDesc",
    icon: BookOpenText,
  },
];
</script>

<template>
  <section>
    <h2 data-setting="settings.mdTheme.title" class="text-base font-semibold">
      {{ t("settings.mdTheme.title") }}
    </h2>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.mdTheme.description") }}</p>
    <div class="mt-4 flex flex-col gap-2">
      <button
        v-for="opt in OPTIONS"
        :key="opt.value"
        type="button"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors hover:bg-accent"
        :class="store.mdTheme === opt.value && 'border-primary'"
        @click="store.setMdTheme(opt.value)"
      >
        <component :is="opt.icon" class="h-4 w-4 shrink-0 text-muted-foreground" />
        <span class="flex-1">
          <span class="block text-sm font-medium">{{ t(opt.labelKey) }}</span>
          <span class="block text-xs text-muted-foreground">{{ t(opt.descriptionKey) }}</span>
        </span>
        <Check v-if="store.mdTheme === opt.value" class="h-4 w-4 shrink-0 text-primary" />
      </button>
    </div>
  </section>
</template>
