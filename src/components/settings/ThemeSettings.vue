<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { computed, onMounted, onUnmounted, ref, type Component } from "vue";
import { Monitor, Moon, Sun } from "@lucide/vue";
import { useSettingsStore, type ThemeMode, type ThemeSkin } from "@/stores/settings";

import ThemeChoicePreview from "./ThemeChoicePreview.vue";

const { t } = useI18n();
const store = useSettingsStore();
const systemDark = window.matchMedia("(prefers-color-scheme: dark)");
const prefersDark = ref(systemDark.matches);
const previewMode = computed(() => {
  if (store.theme !== "system") {
    return store.theme;
  }
  return prefersDark.value ? "dark" : "light";
});

function syncSystemDark(event: MediaQueryListEvent) {
  prefersDark.value = event.matches;
}

onMounted(() => systemDark.addEventListener("change", syncSystemDark));
onUnmounted(() => systemDark.removeEventListener("change", syncSystemDark));

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

const SKINS: { value: ThemeSkin; labelKey: string; descriptionKey: string }[] = [
  {
    value: "default",
    labelKey: "settings.skin.default",
    descriptionKey: "settings.skin.defaultDesc",
  },
  {
    // 设计来源: Animal Island UI https://guokaigdg.github.io/animal-island-ui/#/skill
    value: "island",
    labelKey: "settings.skin.island",
    descriptionKey: "settings.skin.islandDesc",
  },
  {
    // 设计来源: StyleKit Pixel Art https://www.stylekit.top/zh/styles/pixel-art
    value: "pixel",
    labelKey: "settings.skin.pixel",
    descriptionKey: "settings.skin.pixelDesc",
  },
  {
    // 设计来源: https://www.stylekit.top/zh/styles/glassmorphism
    value: "glassmorphism",
    labelKey: "settings.skin.glassmorphism",
    descriptionKey: "settings.skin.glassmorphismDesc",
  },
];
</script>

<template>
  <section>
    <h2 data-setting="settings.general.theme" class="text-base font-semibold">
      {{ t("settings.general.theme") }}
    </h2>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.general.themeDescription") }}</p>
    <div class="mt-4 flex flex-col gap-2">
      <ThemeChoicePreview
        v-for="opt in OPTIONS"
        :key="opt.value"
        :skin="store.themeSkin"
        :mode="opt.value"
        :selected="store.theme === opt.value"
        :icon="opt.icon"
        :label="t(opt.labelKey)"
        :description="t(opt.descriptionKey)"
        @select="store.setTheme(opt.value)"
      />
    </div>

    <h2 data-setting="settings.skin.title" class="mt-8 text-base font-semibold">
      {{ t("settings.skin.title") }}
    </h2>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.skin.description") }}</p>
    <div class="mt-4 flex flex-col gap-2">
      <ThemeChoicePreview
        v-for="skin in SKINS"
        :key="skin.value"
        :skin="skin.value"
        :mode="previewMode"
        :selected="store.themeSkin === skin.value"
        :label="t(skin.labelKey)"
        :description="t(skin.descriptionKey)"
        @select="store.setThemeSkin(skin.value)"
      />
    </div>
  </section>
</template>
