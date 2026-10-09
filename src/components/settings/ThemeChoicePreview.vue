<script setup lang="ts">
import { ref, type Component } from "vue";
import { Check } from "@lucide/vue";
import type { ThemeMode, ThemeSkin } from "@/stores/settings";
import { themePreviewSplit } from "./theme-preview";

const props = defineProps<{
  skin: ThemeSkin;
  mode: ThemeMode;
  selected: boolean;
  label: string;
  description: string;
  icon?: Component;
}>();

const emit = defineEmits<{ select: [] }>();
const split = ref(50);

function updateSplit(event: PointerEvent) {
  if (props.mode !== "system" || event.pointerType === "touch") {
    return;
  }
  const { left, width } = (event.currentTarget as HTMLElement).getBoundingClientRect();
  split.value = themePreviewSplit(event.clientX, left, width);
}
</script>

<template>
  <button
    type="button"
    class="theme-preview theme-preview-choice"
    :class="{ dark: mode === 'dark' }"
    :data-theme="skin"
    :aria-pressed="selected"
    :aria-label="`${label} — ${description}`"
    :style="{ '--theme-split': `${split}%` }"
    @click="emit('select')"
    @pointerenter="updateSplit"
    @pointermove="updateSplit"
    @pointerleave="split = 50"
    @pointercancel="split = 50"
    @blur="split = 50"
  >
    <!-- 两层内容完全对齐,裁剪只改变明暗边界;读屏仅读取按钮自身的标签。 -->
    <span
      v-for="appearance in mode === 'system' ? ['light', 'dark'] : [mode]"
      :key="appearance"
      aria-hidden="true"
      class="theme-preview theme-preview-surface"
      :class="{
        dark: appearance === 'dark',
        'theme-preview-split-light': mode === 'system' && appearance === 'light',
        'theme-preview-split-dark': mode === 'system' && appearance === 'dark',
      }"
      :data-theme="skin"
    >
      <span class="theme-preview-content">
        <span v-if="icon" class="theme-preview-icon">
          <component :is="icon" class="h-4 w-4" />
        </span>
        <span v-else class="theme-preview-swatches">
          <span
            v-for="color in ['background', 'primary', 'foreground']"
            :key="color"
            class="theme-preview-swatch"
            :style="{ backgroundColor: `var(--${color})` }"
          />
        </span>
        <span class="min-w-0 flex-1">
          <span class="block text-sm font-medium">{{ label }}</span>
          <span class="theme-preview-description mt-0.5 block text-xs">{{ description }}</span>
        </span>
        <span class="theme-preview-check" :class="{ 'theme-preview-check-selected': selected }">
          <Check v-if="selected" class="h-3.5 w-3.5" />
        </span>
      </span>
    </span>
    <span v-if="mode === 'system'" aria-hidden="true" class="theme-preview-divider" />
  </button>
</template>

<style scoped>
.theme-preview-choice {
  position: relative;
  isolation: isolate;
  display: grid;
  width: 100%;
  padding: 0;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--background);
  color: var(--foreground);
  font-family: var(--font-app);
  text-align: left;
  cursor: pointer;
  transition:
    border-color 150ms,
    box-shadow 150ms;
}

.theme-preview-choice:hover,
.theme-preview-choice[aria-pressed="true"] {
  border-color: var(--primary);
  box-shadow: 0 0 0 1px var(--primary);
}

.theme-preview-choice:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: 3px;
}

.theme-preview-choice .theme-preview-surface {
  grid-area: 1 / 1;
  min-width: 0;
  background: var(--background);
  color: var(--foreground);
  font-family: var(--font-app);
  border-radius: 0;
}

.theme-preview-content {
  display: flex;
  min-height: 70px;
  align-items: center;
  gap: 12px;
  padding: 12px 14px;
}

.theme-preview-description {
  color: var(--muted-foreground);
}

.theme-preview-choice .theme-preview-icon,
.theme-preview-choice .theme-preview-check {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  border-radius: calc(var(--radius) * 0.65);
}

.theme-preview-icon {
  width: 32px;
  height: 32px;
  background: var(--muted);
  color: var(--foreground);
}

.theme-preview-check {
  width: 22px;
  height: 22px;
  border: 1px solid var(--border);
}

.theme-preview-check-selected {
  border-color: var(--primary);
  background: var(--primary);
  color: var(--primary-foreground);
}

.theme-preview-swatches {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  width: 42px;
}

.theme-preview-choice .theme-preview-swatch {
  width: 18px;
  height: 18px;
  border: 1px solid var(--border);
  border-radius: min(var(--radius), 50%);
}

.theme-preview-swatch + .theme-preview-swatch {
  margin-left: -6px;
}

.theme-preview-split-light {
  clip-path: inset(0 calc(100% - var(--theme-split)) 0 0);
}

.theme-preview-split-dark {
  clip-path: inset(0 0 0 var(--theme-split));
}

.theme-preview-divider {
  position: absolute;
  inset-block: 0;
  left: var(--theme-split);
  width: 1px;
  background: linear-gradient(transparent, rgb(128 128 128 / 45%), transparent);
  pointer-events: none;
}

.theme-preview-choice[data-theme="island"] {
  box-shadow: 0 2px 0 var(--border);
}

.theme-preview-choice[data-theme="island"][aria-pressed="true"],
.theme-preview-choice[data-theme="island"]:hover {
  box-shadow:
    0 2px 0 var(--primary),
    0 0 0 1px var(--primary);
}

.theme-preview-choice[data-theme="pixel"] {
  border-width: 2px;
  box-shadow: 2px 2px 0 var(--pixel-shadow);
  transition: none;
}

.theme-preview-choice[data-theme="pixel"][aria-pressed="true"],
.theme-preview-choice[data-theme="pixel"]:hover {
  border-color: var(--primary);
  box-shadow: 2px 2px 0 var(--primary);
}

.theme-preview-choice[data-theme="pixel"] .theme-preview-surface {
  background-image: repeating-linear-gradient(
    0deg,
    rgb(128 128 128 / 5%) 0 1px,
    transparent 1px 4px
  );
}

.theme-preview-choice[data-theme="glassmorphism"] {
  box-shadow: var(--glass-shadow);
}

.theme-preview-choice[data-theme="glassmorphism"][aria-pressed="true"],
.theme-preview-choice[data-theme="glassmorphism"]:hover {
  box-shadow:
    var(--glass-shadow),
    0 0 0 1px var(--primary);
}

.theme-preview-choice[data-theme="glassmorphism"] .theme-preview-surface {
  background-image: var(--glass-scene);
}

@media (prefers-reduced-motion: reduce) {
  .theme-preview-choice {
    transition: none;
  }
}
</style>
