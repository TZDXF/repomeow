<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { Bot, Maximize2, Minimize2, RotateCcw, X } from "@lucide/vue";
import { Button } from "@/components/ui/button";

defineProps<{
  projectName: string;
  expanded: boolean;
}>();
const emit = defineEmits<{
  newSession: [];
  toggleExpanded: [];
  close: [];
}>();
const { t } = useI18n();
</script>

<template>
  <!-- 头部:标题 + 项目名 + 新会话 + 关闭 -->
  <div class="flex shrink-0 items-center justify-between gap-2 border-b px-3 py-2">
    <div class="flex min-w-0 items-center gap-2">
      <Bot class="h-4 w-4 shrink-0 text-muted-foreground" />
      <span class="shrink-0 text-sm font-semibold">{{ t("chat.title") }}</span>
      <span class="truncate text-xs text-muted-foreground">{{ projectName }}</span>
    </div>
    <div class="flex shrink-0 items-center gap-0.5">
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :title="t('chat.newSession')"
        @click="emit('newSession')"
      >
        <RotateCcw class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :title="expanded ? t('chat.restore') : t('chat.expand')"
        @click="emit('toggleExpanded')"
      >
        <Minimize2 v-if="expanded" class="h-3.5 w-3.5" />
        <Maximize2 v-else class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :title="t('chat.close')"
        @click="emit('close')"
      >
        <X class="h-4 w-4" />
      </Button>
    </div>
  </div>
</template>
