<script setup lang="ts">
import { Check, PanelBottomClose, Power } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { useSettingsStore, type CloseAction } from "@/stores/settings";

const { t } = useI18n();
const store = useSettingsStore();

const OPTIONS: { value: CloseAction; icon: typeof Power; labelKey: string; descKey: string }[] = [
  {
    value: "tray",
    icon: PanelBottomClose,
    labelKey: "settings.tray.closeToTray",
    descKey: "settings.tray.closeToTrayHint",
  },
  {
    value: "exit",
    icon: Power,
    labelKey: "settings.tray.closeToExit",
    descKey: "settings.tray.closeToExitHint",
  },
];
</script>

<template>
  <section>
    <h2 data-setting="settings.tray.title" class="text-base font-semibold">
      {{ t("settings.tray.title") }}
    </h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.tray.description") }}
    </p>
    <div class="mt-4 flex flex-col gap-2">
      <button
        v-for="opt in OPTIONS"
        :key="opt.value"
        type="button"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors hover:bg-accent"
        :class="store.closeAction === opt.value && 'border-primary'"
        @click="store.setCloseAction(opt.value)"
      >
        <component :is="opt.icon" class="h-4 w-4 shrink-0 text-muted-foreground" />
        <span class="flex-1">
          <span class="block text-sm font-medium">{{ t(opt.labelKey) }}</span>
          <span class="block text-xs text-muted-foreground">{{ t(opt.descKey) }}</span>
        </span>
        <Check v-if="store.closeAction === opt.value" class="h-4 w-4 shrink-0 text-primary" />
      </button>
    </div>
  </section>
</template>
