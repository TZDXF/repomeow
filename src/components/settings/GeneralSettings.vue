<script setup lang="ts">
import { onMounted, ref } from "vue";
import { Separator } from "@/components/ui/separator";
import ThemeSettings from "@/components/settings/ThemeSettings.vue";
import MdThemeSettings from "@/components/settings/MdThemeSettings.vue";
import LanguageSettings from "@/components/settings/LanguageSettings.vue";
import OpenWithSettings from "@/components/settings/OpenWithSettings.vue";
import TerminalSettings from "@/components/settings/TerminalSettings.vue";
import WorktreeSettings from "@/components/settings/WorktreeSettings.vue";
import TraySettings from "@/components/settings/TraySettings.vue";
import AutostartSettings from "@/components/settings/AutostartSettings.vue";
import { getTerminalCapabilities, type TerminalCapabilities } from "@/lib/terminal";

const terminalCapabilities = ref<TerminalCapabilities | null>(null);

onMounted(async () => {
  terminalCapabilities.value = await getTerminalCapabilities();
});
</script>

<template>
  <div class="flex flex-col gap-6">
    <ThemeSettings />
    <Separator />
    <MdThemeSettings />
    <Separator />
    <OpenWithSettings />
    <template v-if="terminalCapabilities?.isWindows">
      <Separator />
      <TerminalSettings :availability="terminalCapabilities" />
    </template>
    <Separator />
    <WorktreeSettings />
    <Separator />
    <TraySettings />
    <Separator />
    <LanguageSettings />
    <Separator />
    <AutostartSettings />
  </div>
</template>
