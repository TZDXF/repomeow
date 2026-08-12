<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { ChevronDown } from "@lucide/vue";
import OpenWithIcon from "@/components/open/OpenWithIcon.vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  getEditorAvailability,
  isEditorUnavailable,
  sortOpenWithOptions,
} from "@/lib/open-with";
import type { EditorAvailability } from "@/lib/open-with";
import { cmd } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settings";
import type { EditorKind, Project } from "@/types";

const { t } = useI18n();
// compact(托盘弹窗):与项目页一致的分裂按钮(outline 主按钮 + 相连下拉),主按钮只留图标不展示名称
const props = withDefaults(defineProps<{ project: Project; compact?: boolean }>(), {
  compact: false,
});

const settings = useSettingsStore();

const availability = ref<EditorAvailability | null>(null);

onMounted(async () => {
  availability.value = await getEditorAvailability();
});

// 只展示已扫描到的编辑器;探测中途(null)不过滤,避免闪烁。顺序遵循设置页拖拽结果
const visibleOptions = computed(() =>
  sortOpenWithOptions(settings.openWithOrder).filter(
    (opt) => !isEditorUnavailable(opt.kind, availability.value),
  ),
);

// 默认方式未扫描到时,回退到第一个可用项(explorer / terminal 始终可用)
const current = computed(
  () =>
    visibleOptions.value.find((opt) => opt.kind === settings.defaultOpenWith) ??
    visibleOptions.value[0],
);

async function openWith(kind: EditorKind) {
  try {
    await cmd("open_with", { path: props.project.path, kind });
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <div class="flex items-center">
    <Button
      variant="outline"
      :size="compact ? 'icon-sm' : 'sm'"
      class="rounded-r-none"
      @click.stop="openWith(current.kind)"
    >
      <OpenWithIcon
        :kind="current.kind"
        :icon="current.icon"
        :icon-class="compact ? 'h-3.5 w-3.5' : 'h-4 w-4'"
      />
      <template v-if="!compact">{{ t(current.labelKey) }}</template>
    </Button>
    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button
          variant="outline"
          size="sm"
          :class="compact ? 'rounded-l-none border-l-0 px-1.5' : 'rounded-l-none border-l-0 px-2'"
          @click.stop
        >
          <ChevronDown :class="compact ? 'h-3.5 w-3.5' : 'h-4 w-4'" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" class="w-52" @click.stop>
        <DropdownMenuItem
          v-for="opt in visibleOptions"
          :key="opt.kind"
          class="gap-2 text-xs"
          @click="openWith(opt.kind)"
        >
          <OpenWithIcon :kind="opt.kind" :icon="opt.icon" icon-class="h-3.5 w-3.5" />
          {{ t(opt.descKey) }}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
