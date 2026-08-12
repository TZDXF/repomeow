<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { Check, GripVertical } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { VueDraggable } from "vue-draggable-plus";
import OpenWithIcon from "@/components/open/OpenWithIcon.vue";
import { getEditorAvailability, isEditorUnavailable, sortOpenWithOptions } from "@/lib/open-with";
import type { EditorAvailability, OpenWithOption } from "@/lib/open-with";
import { useSettingsStore } from "@/stores/settings";
import type { EditorKind } from "@/types";

const { t } = useI18n();
const store = useSettingsStore();

// 只展示已扫描到的编辑器;探测中途(null)不过滤,避免闪烁
const availability = ref<EditorAvailability | null>(null);

onMounted(async () => {
  availability.value = await getEditorAvailability();
});

// VueDraggable 直接就地重排的可变列表;由 store 顺序 + 可用性派生
const list = ref<OpenWithOption[]>([]);

// 探测结果 / 外部顺序变化时重建列表(拖拽结束后 list 即最新顺序,重复重建无副作用)
watch(
  [availability, () => store.openWithOrder],
  () => {
    list.value = sortOpenWithOptions(store.openWithOrder).filter(
      (opt) => !isEditorUnavailable(opt.kind, availability.value),
    );
  },
  { immediate: true },
);

// 拖拽结束持久化:可见项按新顺序替换回原完整顺序中的对应位置,未安装的编辑器保持原位
function persistOrder() {
  const visibleKinds = new Set(list.value.map((opt) => opt.kind));
  const queue = list.value.map((opt) => opt.kind);
  let cursor = 0;
  const merged: EditorKind[] = store.openWithOrder.map((kind) =>
    visibleKinds.has(kind) ? queue[cursor++] : kind,
  );
  void store.setOpenWithOrder(merged);
}
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.general.openWith") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.general.openWithDescription") }}
    </p>
    <VueDraggable
      v-model="list"
      :animation="150"
      :force-fallback="true"
      handle=".drag-handle"
      class="mt-4 flex flex-col gap-2"
      @end="persistOrder"
    >
      <button
        v-for="opt in list"
        :key="opt.kind"
        type="button"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors hover:bg-accent"
        :class="store.defaultOpenWith === opt.kind && 'border-primary'"
        @click="store.setDefaultOpenWith(opt.kind)"
      >
        <GripVertical class="drag-handle h-4 w-4 shrink-0 cursor-grab text-muted-foreground" />
        <OpenWithIcon :kind="opt.kind" :icon="opt.icon" icon-class="h-4 w-4 shrink-0" />
        <span class="flex-1 text-sm font-medium">{{ t(opt.labelKey) }}</span>
        <Check v-if="store.defaultOpenWith === opt.kind" class="h-4 w-4 shrink-0 text-primary" />
      </button>
    </VueDraggable>
  </section>
</template>
