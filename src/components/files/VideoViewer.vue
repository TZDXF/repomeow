<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";

const props = defineProps<{
  /** asset 协议 URL(convertFileSrc 产物) */
  src: string;
  name?: string;
}>();

const { t } = useI18n();

/** 解码失败/容器不支持时置位,展示兜底文案 */
const failed = ref(false);

watch(
  () => props.src,
  () => {
    failed.value = false;
  },
);
</script>

<template>
  <div class="flex h-full items-center justify-center overflow-hidden p-4">
    <!-- preload=metadata:只拉取元数据,起播与拖动进度由 asset 协议的 range 请求按需取流 -->
    <video
      v-if="!failed"
      :src="src"
      :title="name"
      controls
      preload="metadata"
      class="max-h-full max-w-full rounded-md bg-black"
      @error="failed = true"
    />
    <p v-else class="text-sm text-destructive">{{ t("files.videoFailed") }}</p>
  </div>
</template>
