<script setup lang="ts">
import { nextTick, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { ChevronDown, ChevronUp, X } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// 文件内查找条(纯 UI,驱动逻辑在 ProjectFiles):
// 输入即查(父组件 watch)、Enter/按钮跳下一个、Shift+Enter 上一个、Esc 关闭;
// 大小写/全字/正则三个开关用文字字形(VS Code 同款 Aa/ab/.*),激活态 bg-accent;
// 右侧计数 current+1/total,非法正则以红框 + 计数位提示。

const text = defineModel<string>("text", { required: true });

defineProps<{
  modes: { caseSensitive: boolean; wholeWord: boolean; useRegex: boolean };
  /** 匹配总数 */
  total: number;
  /** 当前匹配索引,-1 表示无 */
  current: number;
  /** 正则非法 */
  invalid: boolean;
}>();

const emit = defineEmits<{
  (e: "toggle", key: "caseSensitive" | "wholeWord" | "useRegex"): void;
  (e: "next"): void;
  (e: "prev"): void;
  (e: "close"): void;
}>();

const { t } = useI18n();
const host = ref<HTMLElement | null>(null);

onMounted(() => focusInput());

function focusInput() {
  void nextTick(() => host.value?.querySelector("input")?.focus());
}

defineExpose({ focusInput });
</script>

<template>
  <div
    ref="host"
    class="flex shrink-0 items-center gap-1.5 border-b bg-muted/30 px-2 py-1.5"
    @keydown.esc.stop.prevent="emit('close')"
  >
    <Input
      v-model="text"
      :placeholder="t('files.findPlaceholder')"
      class="h-7 w-56 text-sm"
      :class="invalid ? 'border-destructive focus-visible:ring-destructive/30' : ''"
      @keydown.enter.prevent="emit('next')"
      @keydown.enter.shift.prevent="emit('prev')"
    />
    <div class="flex shrink-0 items-center gap-0.5">
      <Button
        v-for="m in [
          { key: 'caseSensitive', label: 'Aa', title: t('files.matchCase') },
          { key: 'wholeWord', label: 'ab', title: t('files.wholeWord') },
          { key: 'useRegex', label: '.*', title: t('files.useRegex') },
        ]"
        :key="m.key"
        variant="ghost"
        size="icon"
        class="h-6 w-7 rounded-sm font-mono text-[11px]"
        :class="modes[m.key] ? 'bg-accent' : 'text-muted-foreground'"
        :title="m.title"
        @click="emit('toggle', m.key)"
      >
        {{ m.label }}
      </Button>
    </div>
    <span class="w-16 shrink-0 text-center font-mono text-xs text-muted-foreground">
      {{
        invalid
          ? t("files.findInvalid")
          : total === 0
            ? t("files.findNoResults")
            : `${current + 1}/${total}`
      }}
    </span>
    <div class="flex shrink-0 items-center gap-0.5">
      <Button
        variant="ghost"
        size="icon"
        class="h-6 w-6"
        :title="t('files.findPrev')"
        :disabled="total === 0"
        @click="emit('prev')"
      >
        <ChevronUp class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-6 w-6"
        :title="t('files.findNext')"
        :disabled="total === 0"
        @click="emit('next')"
      >
        <ChevronDown class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-6 w-6"
        :title="t('files.findClose')"
        @click="emit('close')"
      >
        <X class="h-3.5 w-3.5" />
      </Button>
    </div>
  </div>
</template>
