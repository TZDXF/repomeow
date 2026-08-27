<script setup lang="ts">
import { Check, GitBranch, SquareTerminal, Terminal } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { useSettingsStore, type TerminalKind } from "@/stores/settings";

const { t } = useI18n();
const store = useSettingsStore();

const OPTIONS: { value: TerminalKind; icon: typeof Terminal; labelKey: string; descKey: string }[] =
  [
    {
      value: "cmd",
      icon: Terminal,
      labelKey: "settings.terminal.cmd",
      descKey: "settings.terminal.cmdHint",
    },
    {
      value: "powershell",
      icon: SquareTerminal,
      labelKey: "settings.terminal.powershell",
      descKey: "settings.terminal.powershellHint",
    },
    {
      value: "gitbash",
      icon: GitBranch,
      labelKey: "settings.terminal.gitbash",
      descKey: "settings.terminal.gitbashHint",
    },
  ];
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.terminal.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.terminal.description") }}
    </p>
    <div class="mt-4 flex flex-col gap-2">
      <button
        v-for="opt in OPTIONS"
        :key="opt.value"
        type="button"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors hover:bg-accent"
        :class="store.terminal === opt.value && 'border-primary'"
        @click="store.setTerminal(opt.value)"
      >
        <component :is="opt.icon" class="h-4 w-4 shrink-0 text-muted-foreground" />
        <span class="flex-1">
          <span class="block text-sm font-medium">{{ t(opt.labelKey) }}</span>
          <span class="block text-xs text-muted-foreground">{{ t(opt.descKey) }}</span>
        </span>
        <Check v-if="store.terminal === opt.value" class="h-4 w-4 shrink-0 text-primary" />
      </button>
    </div>
  </section>
</template>
