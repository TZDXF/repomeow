import { describe, expect, it } from "vitest";
import {
  composeMenuEntries,
  resolveComposeMenuAction,
  type ComposeMenuAction,
} from "./compose-menu";

describe("compose action menus", () => {
  it("preserves the file and service context menu order and differences", () => {
    expect(composeMenuEntries("context", false)).toEqual([
      "up",
      "buildUp",
      "build",
      "restart",
      "down",
      "separator",
      "exportContainer",
      "exportImage",
    ]);
    expect(composeMenuEntries("context", true)).toEqual([
      "up",
      "stop",
      "buildUp",
      "build",
      "restart",
      "separator",
      "exportContainer",
      "exportImage",
    ]);
  });

  it("preserves the more menu order for both scopes", () => {
    const expected = ["build", "buildUp", "restart", "separator", "exportContainer", "exportImage"];
    expect(composeMenuEntries("dropdown", false)).toEqual(expected);
    expect(composeMenuEntries("dropdown", true)).toEqual(expected);
  });

  it.each<[ComposeMenuAction, unknown]>([
    ["up", { kind: "run", command: "up -d" }],
    ["stop", { kind: "run", command: "stop" }],
    ["down", { kind: "run", command: "down" }],
    ["buildUp", { kind: "run", command: "up -d --build" }],
    ["build", { kind: "run", command: "build" }],
    ["restart", { kind: "run", command: "restart" }],
    ["exportContainer", { kind: "export", target: "container" }],
    ["exportImage", { kind: "export", target: "image" }],
  ])("routes %s to the same action", (action, expected) => {
    expect(resolveComposeMenuAction(action)).toEqual(expected);
  });
});
