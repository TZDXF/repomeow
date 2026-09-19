<script setup lang="ts">
import { Check } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { useSettingsStore, type Language } from "@/stores/settings";

const { t } = useI18n();
const store = useSettingsStore();

const OPTIONS: { value: Language; labelKey: string; nativeLabelKey: string }[] = [
  {
    value: "zh-CN",
    labelKey: "settings.language.zhCN",
    nativeLabelKey: "settings.language.zhCNNative",
  },
  {
    value: "en-US",
    labelKey: "settings.language.enUS",
    nativeLabelKey: "settings.language.enUSNative",
  },
];
</script>

<template>
  <section>
    <h2 data-setting="settings.general.language" class="text-base font-semibold">
      {{ t("settings.general.language") }}
    </h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.general.languageDescription") }}
    </p>
    <div class="mt-4 flex flex-col gap-2">
      <button
        v-for="opt in OPTIONS"
        :key="opt.value"
        type="button"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors hover:bg-accent"
        :class="store.language === opt.value && 'border-primary'"
        @click="store.setLanguage(opt.value)"
      >
        <span class="flex-1 text-sm font-medium">{{ t(opt.nativeLabelKey) }}</span>
        <Check v-if="store.language === opt.value" class="h-4 w-4 shrink-0 text-primary" />
      </button>
    </div>
  </section>
</template>
