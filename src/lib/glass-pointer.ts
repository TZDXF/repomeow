/** Theme-scoped pointer lighting; only the hovered material is measured per frame. */
export function attachGlassPointer(surface: HTMLElement): () => void {
  const preference = window.matchMedia(
    "(prefers-reduced-motion: reduce), (prefers-reduced-transparency: reduce), (pointer: coarse)",
  );
  let frame = 0;
  let latest: PointerEvent | undefined;
  let active: HTMLElement | null = null;
  const selector = '[data-slot="card"], .project-card, .theme-choice, .chat-fab';

  function clearCard() {
    active?.style.removeProperty("--glass-pointer-x");
    active?.style.removeProperty("--glass-pointer-y");
    active?.removeAttribute("data-glass-pointer");
    active = null;
  }

  function reset() {
    cancelAnimationFrame(frame);
    frame = 0;
    latest = undefined;
    clearCard();
    for (const key of [
      "--glass-scene-x",
      "--glass-scene-y",
      "--glass-shift-x",
      "--glass-shift-y",
      "--glass-pointer-opacity",
    ])
      surface.style.removeProperty(key);
  }

  function paint() {
    frame = 0;
    if (!latest || preference.matches || document.hidden) return;
    const bounds = surface.getBoundingClientRect();
    if (!bounds.width || !bounds.height) return;
    const x = Math.max(0, Math.min(bounds.width, latest.clientX - bounds.left));
    const y = Math.max(0, Math.min(bounds.height, latest.clientY - bounds.top));
    surface.style.setProperty("--glass-scene-x", `${x}px`);
    surface.style.setProperty("--glass-scene-y", `${y}px`);
    surface.style.setProperty("--glass-shift-x", `${(x / bounds.width - 0.5) * 18}px`);
    surface.style.setProperty("--glass-shift-y", `${(y / bounds.height - 0.5) * 14}px`);
    surface.style.setProperty("--glass-pointer-opacity", "1");
    const card =
      latest.target instanceof Element ? latest.target.closest<HTMLElement>(selector) : null;
    const next = card && surface.contains(card) ? card : null;
    if (active !== next) {
      clearCard();
      active = next;
    }
    if (active) {
      const rect = active.getBoundingClientRect();
      active.style.setProperty("--glass-pointer-x", `${latest.clientX - rect.left}px`);
      active.style.setProperty("--glass-pointer-y", `${latest.clientY - rect.top}px`);
      active.setAttribute("data-glass-pointer", "");
    }
  }

  function move(event: PointerEvent) {
    if (preference.matches || event.pointerType === "touch" || document.hidden) return;
    latest = event;
    if (!frame) frame = requestAnimationFrame(paint);
  }

  surface.addEventListener("pointermove", move, { passive: true });
  surface.addEventListener("pointerleave", reset);
  surface.addEventListener("pointercancel", reset);
  surface.addEventListener("scroll", reset, true);
  window.addEventListener("blur", reset);
  document.addEventListener("visibilitychange", reset);
  preference.addEventListener("change", reset);
  return () => {
    reset();
    surface.removeEventListener("pointermove", move);
    surface.removeEventListener("pointerleave", reset);
    surface.removeEventListener("pointercancel", reset);
    surface.removeEventListener("scroll", reset, true);
    window.removeEventListener("blur", reset);
    document.removeEventListener("visibilitychange", reset);
    preference.removeEventListener("change", reset);
  };
}
