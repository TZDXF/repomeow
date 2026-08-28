<script setup lang="ts">
import { Check, GitBranch, SquareTerminal, Terminal } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { Badge } from "@/components/ui/badge";
import type { TerminalCapabilities } from "@/lib/terminal";
import { useSettingsStore, type TerminalKind } from "@/stores/settings";

defineProps<{ availability: TerminalCapabilities }>();

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
    <div class="mt-3 flex items-center gap-2 text-xs text-muted-foreground">
      <span>{{ t("settings.terminal.windowsTerminalHost") }}</span>
      <Badge
        :variant="availability.windowsTerminal ? 'secondary' : 'outline'"
        :class="!availability.windowsTerminal && 'text-muted-foreground'"
      >
        {{
          t(
            availability.windowsTerminal
              ? "settings.terminal.available"
              : "settings.terminal.notDetected",
          )
        }}
      </Badge>
    </div>
    <p
      v-if="store.terminal !== 'cmd' && !availability.shells[store.terminal]"
      class="mt-2 text-xs text-amber-600 dark:text-amber-400"
    >
      {{ t("settings.terminal.selectedUnavailable") }}
    </p>
    <div class="mt-4 flex flex-col gap-2">
      <button
        v-for="opt in OPTIONS"
        :key="opt.value"
        type="button"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-transparent"
        :class="store.terminal === opt.value && 'border-primary'"
        :disabled="!availability.shells[opt.value]"
        @click="store.setTerminal(opt.value)"
      >
        <component :is="opt.icon" class="h-4 w-4 shrink-0 text-muted-foreground" />
        <span class="flex-1">
          <span class="block text-sm font-medium">{{ t(opt.labelKey) }}</span>
          <span class="block text-xs text-muted-foreground">{{ t(opt.descKey) }}</span>
        </span>
        <Badge
          :variant="availability.shells[opt.value] ? 'secondary' : 'outline'"
          :class="!availability.shells[opt.value] && 'text-muted-foreground'"
        >
          {{
            t(
              availability.shells[opt.value]
                ? "settings.terminal.available"
                : "settings.terminal.notDetected",
            )
          }}
        </Badge>
        <Check v-if="store.terminal === opt.value" class="h-4 w-4 shrink-0 text-primary" />
      </button>
    </div>
  </section>
</template>
