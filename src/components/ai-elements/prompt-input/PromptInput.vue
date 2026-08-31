<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import type { PromptInputMessage } from "./types";
import { InputGroup } from "@/components/ui/input-group";
import { cn } from "@/lib/utils";
import { getCurrentInstance, inject, onMounted, onUnmounted } from "vue";
import { usePromptInputProvider } from "./context";
import { PROMPT_INPUT_KEY } from "./types";

const props = defineProps<{
  class?: HTMLAttributes["class"];
  accept?: string;
  multiple?: boolean;
  globalDrop?: boolean;
  maxFiles?: number;
  maxFileSize?: number;
  initialInput?: string;
}>();

const emit = defineEmits<{
  (e: "submit", payload: PromptInputMessage): void;
  (e: "error", payload: { code: string; message: string }): void;
}>();

const instance = getCurrentInstance();

function getListener(name: "onSubmit" | "onError") {
  return instance?.vnode.props?.[name];
}

function callListener<T>(listener: unknown, payload: T) {
  if (Array.isArray(listener)) {
    return Promise.all(listener.map((fn) => (typeof fn === "function" ? fn(payload) : undefined)));
  }

  if (typeof listener === "function") {
    return listener(payload);
  }
}

// --- Dual-mode context handling ---
const inheritedContext = inject(PROMPT_INPUT_KEY, null);
const localContext = inheritedContext
  ? null
  : usePromptInputProvider({
      initialInput: props.initialInput,
      maxFiles: props.maxFiles,
      maxFileSize: props.maxFileSize,
      accept: props.accept,
      onSubmit: (msg) => {
        const listener = getListener("onSubmit");
        if (listener) return callListener(listener, msg);

        emit("submit", msg);
      },
      onError: (err) => {
        const listener = getListener("onError");
        if (listener) {
          void Promise.resolve(callListener(listener, err)).catch((error) => {
            console.error("PromptInput onError listener failed:", error);
          });
          return;
        }

        emit("error", err);
      },
    });

const context = inheritedContext || localContext;

if (!context) {
  throw new Error("PromptInput context is missing.");
}

const { addFiles, submitForm } = context;

function handleDragOver(e: DragEvent) {
  if (e.dataTransfer?.types?.includes("Files")) {
    e.preventDefault();
  }
}

function handleDrop(e: DragEvent) {
  if (e.dataTransfer?.types?.includes("Files")) {
    e.preventDefault();
  }
  if (e.dataTransfer?.files && e.dataTransfer.files.length > 0) {
    addFiles(e.dataTransfer.files);
  }
}

onMounted(() => {
  if (props.globalDrop) {
    document.addEventListener("dragover", handleDragOver);
    document.addEventListener("drop", handleDrop);
  }
});

onUnmounted(() => {
  if (props.globalDrop) {
    document.removeEventListener("dragover", handleDragOver);
    document.removeEventListener("drop", handleDrop);
  }
});

function onFileChange(e: Event) {
  const input = e.target as HTMLInputElement;
  if (input.files) {
    addFiles(input.files);
  }
  input.value = "";
}

function onSubmit(e: Event) {
  e.preventDefault();
  submitForm();
}
</script>

<template>
  <div>
    <input
      :ref="context.fileInputRef"
      type="file"
      class="hidden"
      :accept="accept"
      :multiple="multiple"
      @change="onFileChange"
    />
    <form
      :class="cn('w-full', props.class)"
      @submit="onSubmit"
      @dragover.prevent="handleDragOver"
      @drop.prevent.stop="handleDrop"
    >
      <!-- 抵消 InputGroup 的 has-disabled 整组变灰:组内任一控件禁用(回答中锁定工具栏、
           模型不支持思考等)不应让仍可输入的文本框一起半透明,禁用态由各控件自身表达 -->
      <InputGroup
        class="overflow-hidden has-disabled:bg-transparent! has-disabled:opacity-100! dark:has-disabled:bg-input/30!"
      >
        <slot />
      </InputGroup>
    </form>
  </div>
</template>
