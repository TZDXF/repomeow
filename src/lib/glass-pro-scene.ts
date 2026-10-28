/** 鼠标流光后叠加玻璃折射;静态 CSS 高光继续补充玻璃表面纹理。 */
export const glassProInteractiveLayers = [
  {
    id: "cursor-flow",
    type: "ChromaFlow",
    props: {
      baseColor: "#ae00ff",
      downColor: "#4642ff",
      leftColor: "#5900ff",
      rightColor: "#5b4fff",
      upColor: "#7105ff",
      intensity: 1,
      radius: 3,
      momentum: 30,
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
];
