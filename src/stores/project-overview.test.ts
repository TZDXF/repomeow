import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { cmd } from "@/lib/tauri";
import { useProjectOverviewStore } from "./project-overview";
import type { ProjectOverview } from "@/types";

vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));

describe("external command invalidation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
  });
  it("does not let a response started before the CLI write restore stale commands", async () => {
    let finish!: (data: ProjectOverview) => void;
    const fresh = {
      hidden_items: [{ kind: "customCommand", targetKey: "new" }],
      custom_commands: [],
    } as unknown as ProjectOverview;
    vi.mocked(cmd)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      )
      .mockResolvedValue(fresh);
    const store = useProjectOverviewStore(),
      old = store.refresh(1);
    store.invalidate();
    await store.refresh(1);
    finish({ hidden_items: [], custom_commands: [] });
    expect(await old).toEqual(fresh);
    expect(store.cached(1)).toEqual(fresh);
  });
});
