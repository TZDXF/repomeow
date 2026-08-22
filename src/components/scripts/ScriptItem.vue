<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Eye, EyeOff, Copy, Pencil, Play, Star, Trash2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { commandIcon } from "@/lib/command-icons";
import { copyToClipboard } from "@/lib/utils";

const { t } = useI18n();
const props = defineProps<{
  name: string;
  command: string;
  description?: string;
  icon?: string;
  editable?: boolean;
  /** 是否可被隐藏(显示悬停 EyeOff 按钮) */
  hidable?: boolean;
  /** 当前处于已隐藏状态(灰显 + 常显 Eye 恢复按钮,仅在「显示已隐藏」模式下出现) */
  dimmed?: boolean;
  /** 是否可标记为「常用命令」(显示 Star 按钮,在托盘弹窗中可快速执行) */
  pinnable?: boolean;
  /** 当前是否已被标记 */
  pinned?: boolean;
}>();

const iconComponent = computed(() => (props.icon ? commandIcon(props.icon) : undefined));

const emit = defineEmits<{
  run: [];
  edit: [];
  delete: [];
  toggleHide: [];
  togglePin: [];
}>();

async function copyCommand() {
  await copyToClipboard(props.command);
}
</script>

<template>
  <div
    class="group flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-accent"
    :class="{ 'opacity-50': dimmed }"
  >
    <Button
      variant="ghost"
      size="icon"
      class="h-7 w-7 shrink-0 text-emerald-600"
      :title="t('scripts.item.runTitle', { command })"
      @click="emit('run')"
    >
      <component :is="iconComponent" v-if="iconComponent" class="h-3.5 w-3.5" />
      <Play v-else class="h-3.5 w-3.5" />
    </Button>
    <span class="w-32 shrink-0 truncate text-sm font-medium" :title="description || name">
      {{ name }}
    </span>
    <span class="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground" :title="command">
      {{ command }}
    </span>
    <Button
      v-if="pinnable"
      variant="ghost"
      size="icon"
      class="h-7 w-7 shrink-0"
      :class="pinned ? 'text-yellow-500' : 'hidden group-hover:inline-flex'"
      :title="pinned ? t('pins.unmark') : t('pins.mark')"
      @click="emit('togglePin')"
    >
      <Star class="h-3.5 w-3.5" :class="{ 'fill-yellow-400': pinned }" />
    </Button>
    <Button
      v-if="hidable"
      variant="ghost"
      size="icon"
      class="h-7 w-7 shrink-0"
      :class="dimmed ? 'text-muted-foreground' : 'hidden group-hover:inline-flex'"
      :title="dimmed ? t('common.unhide') : t('common.hide')"
      @click="emit('toggleHide')"
    >
      <Eye v-if="dimmed" class="h-3.5 w-3.5" />
      <EyeOff v-else class="h-3.5 w-3.5" />
    </Button>
    <template v-if="editable">
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7 shrink-0 hidden group-hover:inline-flex"
        :title="t('scripts.item.copy')"
        @click="copyCommand"
      >
        <Copy class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7 shrink-0 hidden group-hover:inline-flex"
        :title="t('scripts.item.edit')"
        @click="emit('edit')"
      >
        <Pencil class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7 shrink-0 hidden group-hover:inline-flex"
        :title="t('scripts.item.delete')"
        @click="emit('delete')"
      >
        <Trash2 class="h-3.5 w-3.5" />
      </Button>
    </template>
  </div>
</template>
