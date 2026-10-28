<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { codeToHtml } from "shiki";

/**
 * 命令输入框:透明 textarea 叠加 Shiki 高亮的 shell 代码层。
 * 长命令自动折行,亮/暗主题经 Shiki 双主题 CSS 变量自动跟随 <html class="dark">。
 */
const model = defineModel<string>({ required: true });
defineProps<{ placeholder?: string }>();

const textareaEl = ref<HTMLTextAreaElement | null>(null);
const layerEl = ref<HTMLDivElement | null>(null);
const highlighted = ref("");
const composing = ref(false);
let seq = 0;

watch(
  () => model.value,
  async (code) => {
    const mySeq = ++seq;
    if (!code.trim()) {
      highlighted.value = "";
      return;
    }
    try {
      const html = await codeToHtml(code, {
        lang: "shell",
        themes: { light: "github-light", dark: "github-dark" },
        defaultColor: false,
      });
      if (mySeq === seq) highlighted.value = html;
    } catch (error) {
      // 高亮失败时保留空 highlighted,模板会回退纯文本层,避免透明文字完全隐形
      if (mySeq === seq) {
        highlighted.value = "";
        console.error("[CommandEditor] Shiki 高亮失败,已回退纯文本", error);
      }
    }
  },
  { immediate: true },
);

/** 高亮层与 textarea 同盒滚动,保证着色/行号与光标行严格对齐 */
function syncScroll() {
  if (!layerEl.value || !textareaEl.value) return;
  layerEl.value.scrollTop = textareaEl.value.scrollTop;
  layerEl.value.scrollLeft = textareaEl.value.scrollLeft;
}

/**
 * 输入法组合期间(如拼音选字):组合文字画在 textarea 上,但 textarea 文字是透明的,
 * 组合拼音会与背景同色看不见。组合期改为:隐藏高亮层 + textarea 用前景色显示原文,
 * 组合结束后恢复透明与着色。
 */
function onCompositionStart() {
  composing.value = true;
}
function onCompositionEnd() {
  composing.value = false;
}
/** 兜底:焦点离开时若组合事件未成对结束,强制恢复高亮态 */
function onBlur() {
  composing.value = false;
}

/** Tab 插入两空格(缩进续行命令),不跳出输入框 */
async function onKeydown(e: KeyboardEvent) {
  if (e.key !== "Tab" || e.shiftKey) return;
  e.preventDefault();
  const ta = textareaEl.value;
  if (!ta) return;
  const start = ta.selectionStart;
  const end = ta.selectionEnd;
  model.value = ta.value.slice(0, start) + "  " + ta.value.slice(end);
  await nextTick();
  ta.focus();
  ta.setSelectionRange(start + 2, start + 2);
  syncScroll();
}
</script>

<template>
  <div
    class="command-editor rounded-lg border border-input bg-transparent transition-colors focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50 dark:bg-input/30"
    :class="{ 'command-editor-composing': composing }"
  >
    <div class="relative">
      <!-- 高亮层:着色随 textarea 滚动;inert 屏蔽 Shiki pre 自带的 tabindex -->
      <div
        ref="layerEl"
        inert
        aria-hidden="true"
        class="command-editor-layer pointer-events-none absolute inset-0 overflow-hidden [scrollbar-gutter:stable]"
      >
        <pre
          v-if="highlighted"
          class="min-w-0 whitespace-pre-wrap break-words py-2 pl-2.5 pr-2.5 font-mono text-sm leading-5"
          v-html="highlighted"
        />
        <!-- 高亮失败时回退纯文本层(与 Shiki pre 同排版),textarea 文字透明,不能让两层同时为空 -->
        <pre
          v-else-if="model"
          class="min-w-0 whitespace-pre-wrap break-words py-2 pl-2.5 pr-2.5 font-mono text-sm leading-5"
          >{{ model }}</pre>
      </div>
      <textarea
        ref="textareaEl"
        v-model="model"
        class="field-sizing-content min-h-16 max-h-48 w-full resize-none overflow-y-auto whitespace-pre-wrap break-words bg-transparent px-2.5 py-2 font-mono text-sm leading-5 text-transparent caret-foreground outline-none placeholder:text-muted-foreground selection:bg-primary/25 [scrollbar-gutter:stable]"
        :placeholder="placeholder"
        spellcheck="false"
        @scroll="syncScroll"
        @keydown="onKeydown"
        @compositionstart="onCompositionStart"
        @compositionend="onCompositionEnd"
        @blur="onBlur"
      />
    </div>
  </div>
</template>

<style scoped>
/* Shiki 双主题产物只在每个 token 上留 --shiki-light/--shiki-dark 变量,由这里按 .dark 切换,
   同时清掉主题自带背景,透出对话框底色。
   注:选择器必须整段包进 :global(),scoped 编译器会丢弃 :global() 之间的中间段 */
:global(.command-editor .shiki),
:global(.command-editor .shiki span) {
  color: var(--shiki-light);
  background-color: transparent;
}

:global(.command-editor .shiki) {
  /* codeToHtml 返回的 <pre class="shiki"> 默认 white-space:pre 会压制折行,必须强制 pre-wrap */
  white-space: pre-wrap;
  overflow-wrap: break-word;
}

:global(html.dark .command-editor .shiki),
:global(html.dark .command-editor .shiki span) {
  color: var(--shiki-dark);
}

/* 输入法组合期间:文字转可见色,高亮层隐藏,避免组合文字透明不可见 */
.command-editor-composing textarea {
  color: var(--color-foreground);
}
.command-editor-composing .command-editor-layer {
  display: none;
}
</style>
