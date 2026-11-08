<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Switch } from "@/components/ui/switch";
import { useSettingsStore } from "@/stores/settings";

const { t } = useI18n(),
  settingsStore = useSettingsStore(),
  autoCheck = computed({
    get: () => settingsStore.autoCheckUpdate,
    set: (v: boolean) => settingsStore.setAutoCheckUpdate(v),
  });
</script>

<template>
  <section>
    <div class="flex items-center justify-between rounded-lg border px-3 py-2.5">
      <div class="flex flex-col gap-0.5">
        <span class="text-sm font-medium">{{ t("settings.update.autoCheck") }}</span>
        <span class="text-xs text-muted-foreground">{{ t("settings.update.autoCheckHint") }}</span>
      </div>
      <Switch v-model="autoCheck" />
    </div>
  </section>
</template>
