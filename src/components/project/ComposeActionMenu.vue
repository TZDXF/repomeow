<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Download, Hammer, ImageDown, Play, RotateCw, Square } from "@lucide/vue";
import { ContextMenuItem } from "@/components/ui/context-menu";
import { ContextMenuSeparator } from "reka-ui";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import {
  composeMenuEntries,
  resolveComposeMenuAction,
  type ComposeCommand,
  type ComposeMenuAction,
} from "./compose-menu";

const props = defineProps<{ kind: "context" | "dropdown"; service: boolean }>();
const emit = defineEmits<{
  run: [command: ComposeCommand];
  export: [kind: "container" | "image"];
}>();
const { t } = useI18n();
const entries = computed(() => composeMenuEntries(props.kind, props.service));
const icons = {
  up: Play,
  stop: Square,
  down: Square,
  buildUp: Hammer,
  build: Hammer,
  restart: RotateCw,
  exportContainer: Download,
  exportImage: ImageDown,
};
const colors: Partial<Record<ComposeMenuAction, string>> = {
  up: "text-emerald-600",
  stop: "text-red-600",
  down: "text-red-600",
  buildUp: "text-emerald-600",
  build: "text-sky-600",
  restart: "text-amber-600",
};

function activate(action: ComposeMenuAction) {
  const resolved = resolveComposeMenuAction(action);
  if (resolved.kind === "export") emit("export", resolved.target);
  else emit("run", resolved.command);
}
</script>

<template>
  <template v-for="(entry, index) in entries" :key="index">
    <component
      :is="kind === 'context' ? ContextMenuSeparator : DropdownMenuSeparator"
      v-if="entry === 'separator'"
    />
    <component
      :is="kind === 'context' ? ContextMenuItem : DropdownMenuItem"
      v-else
      class="gap-2 text-xs"
      v-on="
        kind === 'context' ? { select: () => activate(entry) } : { click: () => activate(entry) }
      "
    >
      <component :is="icons[entry]" class="h-3.5 w-3.5" :class="colors[entry]" />
      {{ t(`docker.${entry}`) }}
    </component>
  </template>
</template>
