<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import type { ShaderInstance } from "shaders/js";
import { attachGlassPointer } from "@/lib/glass-pointer";

let detachPointer: (() => void) | undefined,
  surface: HTMLElement | null = null;

const canvas = ref<HTMLCanvasElement>(),
  ready = ref(false);
let shader: ShaderInstance | undefined,
  observer: MutationObserver | undefined,
  resizeObserver: ResizeObserver | undefined,
  disposed = false,
  generation = 0;

function palette() {
  const dark = document.documentElement.classList.contains("dark");
  // 配色要有足够饱和度差异,否则大平滑场下逐帧颜色变化过小、肉眼不可辨
  return (
    dark
      ? ["#16224a", "#3553c2", "#0e7f8a", "#8a5cc9", "#24365e"]
      : ["#dbe4fb", "#8fabf2", "#7fe0c3", "#c9a5f2", "#f0c884"]
  ).map((color, index) => ({ color, position: index / 4 }));
}

function setReady(value: boolean) {
  ready.value = value;
  if (value) {
    surface?.setAttribute("data-glass-shader-ready", "");
  } else {
    surface?.removeAttribute("data-glass-shader-ready");
  }
}

function destroy() {
  generation++;
  setReady(false);
  shader?.destroy();
  shader = undefined;
}

async function sync() {
  // Pro 始终展示完整动态效果,仅在窗口隐藏时释放 GPU 资源。
  destroy();
  if (disposed || document.hidden || !canvas.value) {
    return;
  }
  const current = generation;
  try {
    const [{ createShader }, { default: mesh }, { default: glass }] = await Promise.all([
      import("shaders/js"),
      import("shaders/core/MeshGradient"),
      import("shaders/core/FlutedGlass"),
    ]);
    if (disposed || current !== generation || !canvas.value) {
      return;
    }
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
          {
            id: "flow-glass",
            type: "FlutedGlass",
            props: {
              aberration: 0.25,
              angle: 30,
              frequency: 8,
              highlightColor: "#ffffff",
              refraction: 1.5,
              softness: 0.5,
            },
          },
        ],
      },
      {
        components: [mesh, glass],
        disableTelemetry: true,
        observeElement: false,
        onReady: () => {
          if (current === generation) {
            setReady(true);
          }
        },
        onError: () => {
          if (current === generation) {
            setReady(false);
          }
        },
      },
    );
    if (disposed || current !== generation) {
      instance.destroy();
      return;
    }
    shader = instance;
    resize();
    setReady(!instance.getFailureReason());
  } catch {
    // The CSS aurora remains visible if initialization or GPU allocation fails.
    if (current === generation) {
      destroy();
    }
  }
}

function resize() {
  if (!canvas.value) {
    return;
  }
  // 按实际显示尺寸渲染,像素比由渲染器处理,不再限制为最长边 1280px。
  const { width, height } = canvas.value.getBoundingClientRect();
  shader?.resize(Math.max(1, Math.round(width)), Math.max(1, Math.round(height)));
  // The library also sets CSS dimensions in resize(); keep the scene stretched
  // to its container rather than shrinking the layout (and retriggering ResizeObserver).
  canvas.value.style.width = "100%";
  canvas.value.style.height = "100%";
}

onMounted(() => {
  surface = canvas.value?.closest<HTMLElement>(".app-surface") ?? null;
  if (surface) {
    detachPointer = attachGlassPointer(surface);
  }
  document.addEventListener("visibilitychange", sync);
  observer = new MutationObserver(() => shader?.update("aurora", { stops: palette() }));
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  resizeObserver = new ResizeObserver(resize);
  if (canvas.value) {
    resizeObserver.observe(canvas.value);
  }
  void sync();
});
onUnmounted(() => {
  detachPointer?.();

  disposed = true;
  destroy();
  observer?.disconnect();
  resizeObserver?.disconnect();
  document.removeEventListener("visibilitychange", sync);
  surface = null;
});
</script>

<template>
  <div class="glass-pro-atmosphere" aria-hidden="true">
    <canvas ref="canvas" :class="{ 'is-ready': ready }" style="width: 100%; height: 100%" />
  </div>
</template>
