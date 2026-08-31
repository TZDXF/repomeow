<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { HoverCardTrigger } from "@/components/ui/hover-card";
import { useContextValue } from "./context";
import ContextIcon from "./ContextIcon.vue";

const { t } = useI18n();
const { usedTokens } = useContextValue();
</script>

<template>
  <HoverCardTrigger as-child>
    <slot v-if="$slots.default" />
    <button
      v-else
      type="button"
      class="text-muted-foreground hover:bg-accent hover:text-foreground flex h-7 items-center rounded-md px-1.5 transition-colors disabled:pointer-events-none disabled:opacity-50"
      :disabled="usedTokens == null"
      :title="t('chat.context.title')"
      v-bind="$attrs"
    >
      <ContextIcon />
    </button>
  </HoverCardTrigger>
</template>
