import { describe, expect, it } from "vitest";
import { glassProInteractiveLayers } from "./glass-pro-scene";

describe("glass pro interactive scene", () => {
  it("composes directional fluid before the glass refraction", () => {
    expect(glassProInteractiveLayers.map((layer) => layer.type)).toEqual([
      "ChromaFlow",
      "FlutedGlass",
    ]);
    const flow = glassProInteractiveLayers[0];
    expect(flow.props).toMatchObject({ intensity: 1, radius: 3, momentum: 30 });
    expect(
      new Set([
        flow.props.upColor,
        flow.props.downColor,
        flow.props.leftColor,
        flow.props.rightColor,
      ]).size,
    ).toBe(4);
  });
});
