import { describe, expect, it } from "vitest";
import { toggleInSet } from "./utils";

describe("toggleInSet", () => {
  it("adds without mutating the original and preserves order", () => {
    const original = new Set(["a", "b"]);
    const result = toggleInSet(original, "c");
    expect([...original]).toEqual(["a", "b"]);
    expect([...result]).toEqual(["a", "b", "c"]);
    expect(result).not.toBe(original);
  });
  it("removes without mutating the original", () => {
    const original = new Set([1, 2]);
    expect([...toggleInSet(original, 1)]).toEqual([2]);
    expect([...original]).toEqual([1, 2]);
  });
});
