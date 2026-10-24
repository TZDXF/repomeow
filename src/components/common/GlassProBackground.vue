<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import type { ShaderInstance } from "shaders/js";
import { attachGlassPointer } from "@/lib/glass-pointer";
import { glassProInteractiveLayers } from "@/lib/glass-pro-scene";

let detachPointer: (() => void) | undefined;

const canvas = ref<HTMLCanvasElement>();
const ready = ref(false);
let shader: ShaderInstance | undefined;
let observer: MutationObserver | undefined;
let resizeObserver: ResizeObserver | undefined;
let motion: MediaQueryList;
let disposed = false;
let generation = 0;

function palette() {
  const dark = document.documentElement.classList.contains("dark");
  // 配色要有足够饱和度差异,否则大平滑场下逐帧颜色变化过小、肉眼不可辨
  return (
    dark
      ? ["#16224a", "#3553c2", "#0e7f8a", "#8a5cc9", "#24365e"]
      : ["#dbe4fb", "#8fabf2", "#7fe0c3", "#c9a5f2", "#f0c884"]
  ).map((color, index) => ({ color, position: index / 4 }));
}

function destroy() {
  generation++;
  ready.value = false;
  shader?.destroy();
  shader = undefined;
}

async function sync() {
  // 隐藏窗口与减少动态效果偏好不分配 GPU 资源;注意不能用
  // prefers-reduced-transparency 做门槛——Windows 关闭"透明效果"时它会命中,
  // 但流动背景本身不是透明效果,不应被禁用。
  destroy();
  if (disposed || document.hidden || motion.matches || !canvas.value) return;
  const current = generation;
  try {
    const [
      { createShader, getWebGPUSupport },
      { default: mesh },
      { default: flow },
      { default: glass },
    ] = await Promise.all([
      import("shaders/js"),
      import("shaders/core/MeshGradient"),
      import("shaders/core/ChromaFlow"),
      import("shaders/core/FlutedGlass"),
    ]);
    if (disposed || current !== generation || !(await getWebGPUSupport()).supported) return;
    if (disposed || current !== generation || !canvas.value) return;
    const instance = await createShader(
      canvas.value,
      {
        components: [
          {
            id: "aurora",
            type: "MeshGradient",
            props: {
              stops: palette(),
              speed: 0.6,
              count: 6,
              smoothness: 1.6,
              variation: 0.5,
              swirl: 0.7,
              drift: 0.55,
              wrapping: 0,
              seed: 8,
            },
          },
          ...glassProInteractiveLayers,
        ],
      },
      {
        components: [mesh, flow, glass],
        disableTelemetry: true,
        observeElement: false,
        onReady: () => {
          if (current === generation) ready.value = true;
        },
        onError: () => {
          if (current === generation) ready.value = false;
        },
      },
    );
    if (disposed || current !== generation) {
      instance.destroy();
      return;
    }
    shader = instance;
    resize();
    ready.value = !instance.getFailureReason();
  } catch {
    // The CSS aurora remains visible if initialization or GPU allocation fails.
    if (current === generation) destroy();
  }
}

function resize() {
  if (!canvas.value) return;
  // Soft gradients need no retina-resolution buffer; cap GPU fill cost.
  const { width, height } = canvas.value.getBoundingClientRect();
  const scale = Math.min(1, 1280 / Math.max(width, height));
  shader?.resize(Math.max(1, Math.round(width * scale)), Math.max(1, Math.round(height * scale)));
  // The library also sets CSS dimensions in resize(); keep the scene stretched
  // to its container rather than shrinking the layout (and retriggering ResizeObserver).
  canvas.value.style.width = "100%";
  canvas.value.style.height = "100%";
}

onMounted(() => {
  const surface = canvas.value?.closest<HTMLElement>(".app-surface");
  if (surface) detachPointer = attachGlassPointer(surface);
  motion = window.matchMedia("(prefers-reduced-motion: reduce)");
  motion.addEventListener("change", sync);
  document.addEventListener("visibilitychange", sync);
  observer = new MutationObserver(() => shader?.update("aurora", { stops: palette() }));
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  resizeObserver = new ResizeObserver(resize);
  if (canvas.value) resizeObserver.observe(canvas.value);
  void sync();
});
onUnmounted(() => {
  detachPointer?.();

  disposed = true;
  destroy();
  observer?.disconnect();
  resizeObserver?.disconnect();
  motion?.removeEventListener("change", sync);
  document.removeEventListener("visibilitychange", sync);
});
</script>

<template>
  <div class="glass-pro-atmosphere" aria-hidden="true">
    <canvas ref="canvas" :class="{ 'is-ready': ready }" style="width: 100%; height: 100%" />
  </div>
</template>
