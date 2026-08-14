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
  openProjectWith,
  sortOpenWithOptions,
} from "@/lib/open-with";
import type { EditorAvailability, OpenWithOption } from "@/lib/open-with";
import { useSettingsStore } from "@/stores/settings";
import type { Project } from "@/types";

const { t } = useI18n();
const props = withDefaults(defineProps<{ project: Project; compact?: boolean }>(), {
  compact: false,
});

const settings = useSettingsStore();
const availability = ref<EditorAvailability | null>(null);

onMounted(async () => {
  availability.value = await getEditorAvailability();
});

const visibleOptions = computed(() =>
  sortOpenWithOptions(settings.openWithOrder, settings.customOpenWith).filter(
    (option) => !isEditorUnavailable(option, availability.value),
  ),
);

const current = computed(
  () =>
    visibleOptions.value.find((option) => option.id === settings.defaultOpenWith) ??
    visibleOptions.value[0],
);

function optionLabel(option: OpenWithOption): string {
  return option.custom ? option.name : t(option.labelKey);
}

async function openWith(option: OpenWithOption) {
  try {
    await openProjectWith(option, props.project.path);
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <div class="flex items-center">
    <Button
      v-if="current"
      variant="outline"
      :size="compact ? 'icon-sm' : 'sm'"
      class="rounded-r-none"
      @click.stop="openWith(current)"
    >
      <OpenWithIcon
        :kind="current.custom ? undefined : current.kind"
        :icon="current.icon"
        :icon-class="compact ? 'h-3.5 w-3.5' : 'h-4 w-4'"
      />
      <template v-if="!compact">{{ optionLabel(current) }}</template>
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
          v-for="option in visibleOptions"
          :key="option.id"
          class="gap-2 text-xs"
          @click="openWith(option)"
        >
          <OpenWithIcon
            :kind="option.custom ? undefined : option.kind"
            :icon="option.icon"
            icon-class="h-3.5 w-3.5"
          />
          {{ optionLabel(option) }}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
