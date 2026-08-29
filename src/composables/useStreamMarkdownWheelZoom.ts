import { onMounted, onUnmounted } from "vue";

// vue-stream-markdown 的缩放容器只在 ctrl/meta + 滚轮时缩放,且 wheelSensitivity 为 0.01
// (一格滚轮 deltaY≈100 即 ±1 倍,步长过大)。全屏预览里希望直接滚轮缩放:
// 在捕获阶段拦截全屏弹窗内缩放容器的滚轮事件,改派发一个带 ctrlKey、deltaY 缩小的
// 合成事件,复用库自身的缩放逻辑(以鼠标位置为焦点)。
const ZOOM_CONTAINER_SELECTOR = `[data-stream-markdown="modal"] [data-stream-markdown="zoom-container"]`;
const WHEEL_DELTA_SCALE = 0.2;

export function useStreamMarkdownWheelZoom() {
  function onWheel(event: WheelEvent) {
    // 合成事件自带 ctrlKey,据此直接返回,避免递归;原生 ctrl+滚轮/触控板捏合也走库原逻辑
    if (event.ctrlKey || event.metaKey) return;
    // 用 Element 而非 HTMLElement:事件目标通常是 mermaid SVG 内部的 SVGElement
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (!target.closest(ZOOM_CONTAINER_SELECTOR)) return;
    event.preventDefault();
    event.stopPropagation();
    target.dispatchEvent(
      new WheelEvent("wheel", {
        deltaY: event.deltaY * WHEEL_DELTA_SCALE,
        deltaX: event.deltaX,
        deltaMode: event.deltaMode,
        clientX: event.clientX,
        clientY: event.clientY,
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
  }

  onMounted(() => {
    window.addEventListener("wheel", onWheel, { capture: true, passive: false });
  });
  onUnmounted(() => {
    window.removeEventListener("wheel", onWheel, { capture: true });
  });
}
