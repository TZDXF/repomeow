<script setup lang="ts">
import { useI18n } from "vue-i18n";
import type { Component } from "vue";
import { Check, Monitor, Moon, Sun } from "@lucide/vue";
import { useSettingsStore, type ThemeMode, type ThemeSkin } from "@/stores/settings";

const { t } = useI18n();
const store = useSettingsStore();

const OPTIONS: { value: ThemeMode; labelKey: string; descriptionKey: string; icon: Component }[] = [
  {
    value: "system",
    labelKey: "settings.theme.system",
    descriptionKey: "settings.theme.systemDesc",
    icon: Monitor,
  },
  {
    value: "light",
    labelKey: "settings.theme.light",
    descriptionKey: "settings.theme.lightDesc",
    icon: Sun,
  },
  {
    value: "dark",
    labelKey: "settings.theme.dark",
    descriptionKey: "settings.theme.darkDesc",
    icon: Moon,
  },
];

// 色点顺序: 背景 / 主色 / 文字
const SKINS: { value: ThemeSkin; labelKey: string; descriptionKey: string; swatches: string[] }[] =
  [
    {
      value: "default",
      labelKey: "settings.skin.default",
      descriptionKey: "settings.skin.defaultDesc",
      swatches: ["#ffffff", "#171717", "#525252"],
    },
    {
      value: "island",
      labelKey: "settings.skin.island",
      descriptionKey: "settings.skin.islandDesc",
      swatches: ["#f8f8f0", "#19c8b9", "#794f27"],
    },
    {
      value: "glass",
      labelKey: "settings.skin.glass",
      descriptionKey: "settings.skin.glassDesc",
      swatches: ["#070d1f", "#22d3ee", "#8b5cf6"],
    },
    {
      value: "pixel",
      labelKey: "settings.skin.pixel",
      descriptionKey: "settings.skin.pixelDesc",
      swatches: ["#f4f4f4", "#ff004d", "#1a1c2c"],
    },
  ];
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.general.theme") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.general.themeDescription") }}</p>
    <div class="mt-4 flex flex-col gap-2">
      <button
        v-for="opt in OPTIONS"
        :key="opt.value"
        type="button"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors hover:bg-accent"
        :class="store.theme === opt.value && 'border-primary'"
        @click="store.setTheme(opt.value)"
      >
        <component :is="opt.icon" class="h-4 w-4 shrink-0 text-muted-foreground" />
        <span class="flex-1">
          <span class="block text-sm font-medium">{{ t(opt.labelKey) }}</span>
          <span class="block text-xs text-muted-foreground">{{ t(opt.descriptionKey) }}</span>
        </span>
        <Check v-if="store.theme === opt.value" class="h-4 w-4 shrink-0 text-primary" />
      </button>
    </div>

    <h2 class="mt-8 text-base font-semibold">{{ t("settings.skin.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.skin.description") }}</p>
    <div class="mt-4 flex flex-col gap-2">
      <button
        v-for="skin in SKINS"
        :key="skin.value"
        type="button"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors hover:bg-accent"
        :class="store.themeSkin === skin.value && 'border-primary'"
        @click="store.setThemeSkin(skin.value)"
      >
        <span class="flex shrink-0 items-center -space-x-1.5">
          <span
            v-for="color in skin.swatches"
            :key="color"
            class="h-4 w-4 rounded-full border border-black/10"
            :style="{ backgroundColor: color }"
          />
        </span>
        <span class="flex-1">
          <span class="block text-sm font-medium">{{ t(skin.labelKey) }}</span>
          <span class="block text-xs text-muted-foreground">{{ t(skin.descriptionKey) }}</span>
        </span>
        <Check v-if="store.themeSkin === skin.value" class="h-4 w-4 shrink-0 text-primary" />
      </button>
    </div>
  </section>
</template>
