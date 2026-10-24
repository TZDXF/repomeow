import { afterEach, describe, expect, it, vi } from "vitest";
import { attachGlassPointer } from "./glass-pointer";

afterEach(() => vi.unstubAllGlobals());

describe("glass pointer lifecycle", () => {
  it("coalesces pointer events and cleans up styles and listeners", () => {
    const preference = Object.assign(new EventTarget(), { matches: false });
    const win = Object.assign(new EventTarget(), { matchMedia: () => preference });
    const doc = Object.assign(new EventTarget(), { hidden: false });
    const values = new Map<string, string>();
    const surface = Object.assign(new EventTarget(), {
      style: {
        setProperty: (key: string, value: string) => values.set(key, value),
        removeProperty: (key: string) => values.delete(key),
      },
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 100 }),
    });
    let render: (() => void) | undefined;
    const raf = vi.fn((callback: () => void) => {
      render = callback;
      return 1;
    });
    vi.stubGlobal("window", win);
    vi.stubGlobal("document", doc);
    vi.stubGlobal(
      "Element",
      class {
        readonly nodeType = 1;
      },
    );
    vi.stubGlobal("requestAnimationFrame", raf);
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    const detach = attachGlassPointer(surface as unknown as HTMLElement);
    const move = () =>
      surface.dispatchEvent(
        Object.assign(new Event("pointermove"), {
          clientX: 75,
          clientY: 25,
          pointerType: "mouse",
        }),
      );
    move();
    move();
    expect(raf).toHaveBeenCalledTimes(1);
    render?.();
    expect(values.get("--glass-scene-x")).toBe("75px");
    expect(values.get("--glass-shift-x")).toBe("4.5px");
    preference.matches = true;
    preference.dispatchEvent(new Event("change"));
    expect(values.size).toBe(0);
    move();
    expect(raf).toHaveBeenCalledTimes(1);
    preference.matches = false;
    detach();
    move();
    expect(raf).toHaveBeenCalledTimes(1);
    expect(values.size).toBe(0);
  });
});
