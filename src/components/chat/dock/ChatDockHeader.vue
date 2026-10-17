<script setup lang="ts">
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { Check, History, RotateCcw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Popover, PopoverTrigger } from "@/components/ui/popover";
import { PopoverContent } from "@/components/common/project-floating-content";
import ScrollArea from "@/components/common/ScrollArea.vue";
import { formatLocalDateTime } from "@/lib/format";
import type { ChatHistoryEntry } from "@/lib/chat";

defineProps<{
  entries: ChatHistoryEntry[];
  activeId: string | null;
  disabled: boolean;
}>();
const emit = defineEmits<{
  newSession: [];
  selectHistory: [id: string];
}>();
const { t } = useI18n();
const historyOpen = ref(false);

function selectHistory(id: string) {
  historyOpen.value = false;
  emit("selectHistory", id);
}
</script>

<template>
  <div class="flex shrink-0 items-center justify-end gap-0.5 border-b px-3 py-2">
    <Button
      variant="ghost"
      size="icon"
      class="h-7 w-7"
      :disabled="disabled"
      :title="t('chat.newSession')"
      :aria-label="t('chat.newSession')"
      @click="emit('newSession')"
    >
      <RotateCcw class="h-3.5 w-3.5" />
    </Button>
    <Popover v-model:open="historyOpen">
      <PopoverTrigger as-child>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :disabled="disabled"
          :title="t('chat.historyTitle')"
          :aria-label="t('chat.historyTitle')"
        >
          <History class="h-3.5 w-3.5" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" class="w-80 p-1">
        <p class="px-2 py-2 text-sm font-medium">{{ t("chat.historyTitle") }}</p>
        <p v-if="!entries.length" class="px-2 py-4 text-center text-xs text-muted-foreground">
          {{ t("chat.historyEmpty") }}
        </p>
        <ScrollArea v-else class="max-h-80">
          <button
            v-for="entry in entries"
            :key="entry.id"
            type="button"
            class="flex w-full items-center gap-2 rounded-sm px-2 py-2 text-left hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
            :aria-current="entry.id === activeId ? 'true' : undefined"
            :title="entry.title"
            @click="selectHistory(entry.id)"
          >
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm">{{ entry.title }}</p>
              <p class="mt-0.5 text-xs text-muted-foreground">
                {{ formatLocalDateTime(entry.updatedAt) }}
              </p>
            </div>
            <Check v-if="entry.id === activeId" class="size-3.5 shrink-0 text-primary" />
          </button>
        </ScrollArea>
      </PopoverContent>
    </Popover>
  </div>
</template>
