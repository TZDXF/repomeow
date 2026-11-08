<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { ExternalLink, RefreshCw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import UpdateSettings from "@/components/settings/UpdateSettings.vue";
import { useUpdateStore } from "@/stores/update";

const { t } = useI18n(),
  updateStore = useUpdateStore();

function onCheck() {
  if (updateStore.hasUpdate) {
    updateStore.dialogOpen = true;
  } else {
    updateStore.checkForUpdate(true);
  }
}
</script>

<template>
  <div class="flex flex-col gap-6">
    <section>
      <h2 class="text-base font-semibold">{{ t("settings.about.title") }}</h2>
      <div class="mt-4 flex items-center gap-3">
        <div class="flex flex-col gap-0.5">
          <span class="text-base font-medium">{{ t("app.title") }}</span>
          <span class="text-xs text-muted-foreground">{{ t("app.name") }}</span>
        </div>
        <span class="rounded-md bg-muted px-2 py-0.5 font-mono text-xs">
          v{{ updateStore.currentVersion || "-" }}
        </span>
        <Button
          variant="outline"
          size="sm"
          :disabled="updateStore.status === 'checking' || updateStore.status === 'downloading'"
          @click="onCheck"
        >
          <RefreshCw v-if="updateStore.status === 'checking'" class="h-3.5 w-3.5 animate-spin" />
          <template v-else>{{ t("update.check") }}</template>
        </Button>
        <span v-if="updateStore.hasUpdate" class="text-sm text-primary">
          {{ t("update.available", { version: updateStore.update?.version ?? "" }) }}
        </span>
      </div>
      <p class="mt-1 text-sm text-muted-foreground">
        {{ t("settings.about.description") }}
      </p>
      <a
        href="https://github.com/TZDXF/repomeow"
        target="_blank"
        rel="noopener noreferrer"
        class="mt-2 inline-flex items-center gap-1.5 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ExternalLink class="h-3.5 w-3.5" />
        <span>{{ t("app.repo") }}</span>
      </a>
    </section>
    <Separator />
    <UpdateSettings />
  </div>
</template>
