<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { Bot } from "@lucide/vue";
import { Button } from "@/components/ui/button";

// ── 空会话占位:引导文案 + 建议提问 + 未配置提示 ──
defineProps<{
  aiReady: boolean;
}>();
const emit = defineEmits<{
  send: [text: string];
}>();
const { t } = useI18n();
const router = useRouter();

const suggestions = computed(() => [
  t("chat.suggestion1"),
  t("chat.suggestion2"),
  t("chat.suggestion3"),
]);
</script>

<template>
  <div class="flex flex-col items-center gap-3 text-center">
    <Bot class="size-8 text-muted-foreground" />
    <div class="space-y-1">
      <h3 class="text-sm font-medium">{{ t("chat.emptyTitle") }}</h3>
      <p class="text-muted-foreground text-sm">{{ t("chat.emptyHint") }}</p>
    </div>
    <div class="flex flex-wrap justify-center gap-2">
      <Button
        v-for="suggestion in suggestions"
        :key="suggestion"
        variant="outline"
        size="sm"
        :disabled="!aiReady"
        @click="emit('send', suggestion)"
      >
        {{ suggestion }}
      </Button>
    </div>
    <p v-if="!aiReady" class="text-muted-foreground text-xs">
      {{ t("chat.notConfigured") }}
      <Button variant="link" size="sm" class="h-auto p-0 text-xs" @click="router.push('/settings')">
        {{ t("chat.goSettings") }}
      </Button>
    </p>
  </div>
</template>
