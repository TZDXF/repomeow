<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Maximize, ZoomIn, ZoomOut } from "@lucide/vue";
import { Button } from "@/components/ui/button";

const props = defineProps<{
  src: string;
  alt?: string;
  /** svg 文件:无固有尺寸时回退解析 viewBox 得到像素标注 */
  svg?: boolean;
}>();

const { t } = useI18n();

const MIN_SCALE = 0.05;
const MAX_SCALE = 8;
const ZOOM_STEP = 1.25;

const viewport = ref<HTMLDivElement | null>(null);
const dims = ref<{ w: number; h: number } | null>(null);
const loadFailed = ref(false);
/** null = 适应窗口;数字 = 相对原始像素的缩放比例(1 = 实际大小) */
const scale = ref<number | null>(null);
const tx = ref(0);
const ty = ref(0);
const dragging = ref(false);

// 视口内容区尺寸(不含 p-4 内边距),随窗口缩放/左栏拖宽联动
const vpW = ref(0);
const vpH = ref(0);
let resizeObserver: ResizeObserver | undefined;

onMounted(() => {
  resizeObserver = new ResizeObserver((entries) => {
    const rect = entries[0]?.contentRect;
    vpW.value = rect?.width ?? 0;
    vpH.value = rect?.height ?? 0;
    clampPan();
  });
  if (viewport.value) resizeObserver.observe(viewport.value);
});

onBeforeUnmount(() => resizeObserver?.disconnect());

/** 适应窗口模式下的等比缩放比例(不放大超过原始像素) */
const fitScale = computed(() => {
  const d = dims.value;
  if (!d || d.w === 0 || d.h === 0 || vpW.value === 0 || vpH.value === 0) return 1;
  return Math.min(1, vpW.value / d.w, vpH.value / d.h);
});

function currentScale(): number {
  return scale.value ?? fitScale.value;
}

const percentLabel = computed(() => `${Math.round(currentScale() * 100)}%`);

/** 缩放后超出视口的图像部分保持可拖回:平移量钳制在溢出半宽内 */
function clampPan() {
  const d = dims.value;
  const s = scale.value;
  if (s === null || !d) {
    tx.value = 0;
    ty.value = 0;
    return;
  }
  const maxX = Math.max(0, (d.w * s - vpW.value) / 2);
  const maxY = Math.max(0, (d.h * s - vpH.value) / 2);
  tx.value = Math.min(maxX, Math.max(-maxX, tx.value));
  ty.value = Math.min(maxY, Math.max(-maxY, ty.value));
}

const pannable = computed(() => {
  const d = dims.value;
  const s = scale.value;
  if (s === null || !d) return false;
  return d.w * s > vpW.value + 1 || d.h * s > vpH.value + 1;
});

/** 以光标为焦点的缩放:缩放前后保持光标下的图像点位不动 */
function zoomAt(clientX: number, clientY: number, factor: number) {
  const vp = viewport.value;
  if (!vp || !dims.value || !factor) return;
  const rect = vp.getBoundingClientRect();
  const cx = clientX - rect.left - rect.width / 2;
  const cy = clientY - rect.top - rect.height / 2;
  const s0 = currentScale();
  const s1 = Math.min(MAX_SCALE, Math.max(MIN_SCALE, s0 * factor));
  tx.value = cx - (cx - tx.value) * (s1 / s0);
  ty.value = cy - (cy - ty.value) * (s1 / s0);
  scale.value = s1;
  clampPan();
}

function zoomStep(factor: number) {
  const rect = viewport.value?.getBoundingClientRect();
  if (!rect) return;
  zoomAt(rect.left + rect.width / 2, rect.top + rect.height / 2, factor);
}

function setFit() {
  scale.value = null;
  tx.value = 0;
  ty.value = 0;
}

function showActual() {
  scale.value = 1;
  tx.value = 0;
  ty.value = 0;
}

/** 视口本身无可滚动内容,滚轮一律用于缩放(触控板双指捏合同样走 wheel) */
function onWheel(e: WheelEvent) {
  if (!dims.value || !e.deltaY) return;
  e.preventDefault();
  zoomAt(e.clientX, e.clientY, e.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP);
}

function onDblClick() {
  if (scale.value === null) showActual();
  else setFit();
}

function onPointerDown(e: PointerEvent) {
  if (e.button !== 0 || !pannable.value) return;
  const target = e.currentTarget as HTMLElement;
  target.setPointerCapture(e.pointerId);
  const startX = e.clientX - tx.value;
  const startY = e.clientY - ty.value;
  dragging.value = true;
  const onMove = (ev: PointerEvent) => {
    tx.value = ev.clientX - startX;
    ty.value = ev.clientY - startY;
    clampPan();
  };
  const onUp = () => {
    dragging.value = false;
    target.removeEventListener("pointermove", onMove);
    target.removeEventListener("pointerup", onUp);
    target.removeEventListener("pointercancel", onUp);
  };
  target.addEventListener("pointermove", onMove);
  target.addEventListener("pointerup", onUp);
  target.addEventListener("pointercancel", onUp);
}

let viewBoxSeq = 0;

function onImgLoad(e: Event) {
  loadFailed.value = false;
  const img = e.target as HTMLImageElement;
  if (img.naturalWidth > 0 && img.naturalHeight > 0) {
    dims.value = { w: img.naturalWidth, h: img.naturalHeight };
    return;
  }
  // svg 未写 width/height 属性时浏览器报 0,回退解析 viewBox 标注像素
  dims.value = null;
  if (props.svg) parseViewBox();
}

async function parseViewBox() {
  const mySeq = ++viewBoxSeq;
  try {
    const text = await (await fetch(props.src)).text();
    const m = text.match(/viewBox\s*=\s*["']\s*[\d.+-]+[ ,]+[\d.+-]+[ ,]+([\d.]+)[ ,]+([\d.]+)/i);
    const w = m ? Number(m[1]) : 0;
    const h = m ? Number(m[2]) : 0;
    if (mySeq === viewBoxSeq && w > 0 && h > 0) {
      dims.value = { w: Math.round(w), h: Math.round(h) };
    }
  } catch {
    // 读取失败仅少一个像素标注,不影响展示
  }
}

function onImgError() {
  loadFailed.value = true;
  dims.value = null;
}

watch(
  () => props.src,
  () => {
    scale.value = null;
    tx.value = 0;
    ty.value = 0;
    dims.value = null;
    loadFailed.value = false;
    viewBoxSeq++;
  },
);
</script>

<template>
  <div
    ref="viewport"
    class="relative h-full overflow-hidden p-4"
    :title="t('files.zoomHint')"
    @wheel="onWheel"
  >
    <div class="flex h-full items-center justify-center">
      <img
        v-if="!loadFailed"
        :src="src"
        :alt="alt"
        class="shrink-0 select-none object-contain"
        :class="[
          scale === null ? 'max-h-full max-w-full' : '',
          pannable ? (dragging ? 'cursor-grabbing' : 'cursor-grab') : '',
        ]"
        :style="
          scale === null
            ? undefined
            : {
                width: `${(dims?.w ?? 0) * scale}px`,
                transform: `translate(${tx}px, ${ty}px)`,
              }
        "
        draggable="false"
        @load="onImgLoad"
        @error="onImgError"
        @dblclick="onDblClick"
        @pointerdown="onPointerDown"
      />
      <p v-else class="text-sm text-destructive">{{ t("files.loadFailed") }}</p>
    </div>

    <!-- 缩放与像素标注工具条 -->
    <div
      v-if="dims && !loadFailed"
      class="absolute bottom-3 left-1/2 flex max-w-[calc(100%-2rem)] -translate-x-1/2 flex-wrap items-center justify-center gap-0.5 rounded-lg border bg-background/90 p-1 shadow-md"
    >
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :disabled="currentScale() <= MIN_SCALE + 1e-3"
        :title="t('files.zoomOut')"
        @click="zoomStep(1 / ZOOM_STEP)"
      >
        <ZoomOut class="h-3.5 w-3.5" />
      </Button>
      <button
        class="h-7 min-w-12 rounded-md px-1 text-xs tabular-nums text-muted-foreground hover:bg-accent"
        :title="scale === null ? t('files.zoomActual') : t('files.zoomFit')"
        @click="onDblClick"
      >
        {{ percentLabel }}
      </button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :disabled="currentScale() >= MAX_SCALE - 1e-3"
        :title="t('files.zoomIn')"
        @click="zoomStep(ZOOM_STEP)"
      >
        <ZoomIn class="h-3.5 w-3.5" />
      </Button>
      <span class="mx-1 h-4 w-px bg-border" />
      <span class="px-1 text-xs tabular-nums text-muted-foreground">
        {{ Math.round(dims.w) }} × {{ Math.round(dims.h) }}
      </span>
      <span class="mx-1 h-4 w-px bg-border" />
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :class="scale === null ? 'bg-accent' : ''"
        :title="t('files.zoomFit')"
        @click="setFit"
      >
        <Maximize class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7 text-xs"
        :class="scale === 1 ? 'bg-accent' : ''"
        :title="t('files.zoomActual')"
        @click="showActual"
      >
        1:1
      </Button>
    </div>
  </div>
</template>
