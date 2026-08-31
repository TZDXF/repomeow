<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { CollapsibleContent } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { computed, useSlots } from "vue";
import { MessageResponse } from "@/components/ai-elements/message";

interface Props {
  class?: HTMLAttributes["class"];
  content: string;
  /** 同 MessageResponse:已完成内容用 static,仅流式累积中的思考传 streaming */
  mode?: "static" | "streaming";
}

const props = withDefaults(defineProps<Props>(), { mode: "static" });
const slots = useSlots();

const slotContent = computed<string | undefined>(() => {
  const nodes = slots.default?.();
  if (!Array.isArray(nodes)) return undefined;
  let text = "";
  for (const node of nodes) {
    if (typeof node.children === "string") text += node.children;
  }
  return text || undefined;
});

const md = computed(() => (slotContent.value ?? props.content ?? "") as string);
</script>

<template>
  <CollapsibleContent
    :class="
      cn(
        'mt-2 text-xs',
        'data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-top-2',
        'data-[state=open]:slide-in-from-top-2 text-muted-foreground',
        'outline-none data-[state=closed]:animate-out data-[state=open]:animate-in',
        props.class,
      )
    "
  >
    <MessageResponse :content="md" :mode="mode" />
  </CollapsibleContent>
</template>
