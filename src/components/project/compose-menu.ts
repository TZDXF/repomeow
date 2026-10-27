export type ComposeMenuAction =
  | "up"
  | "stop"
  | "down"
  | "buildUp"
  | "build"
  | "restart"
  | "exportContainer"
  | "exportImage";
export type ComposeMenuEntry = ComposeMenuAction | "separator";

/** 顺序与操作范围分别对应文件/服务的右键菜单和更多菜单。 */
export function composeMenuEntries(
  kind: "context" | "dropdown",
  service: boolean,
): ComposeMenuEntry[] {
  if (kind === "dropdown") {
    return ["build", "buildUp", "restart", "separator", "exportContainer", "exportImage"];
  }
  return [
    "up",
    ...(service ? ["stop" as const] : []),
    "buildUp",
    "build",
    "restart",
    ...(!service ? ["down" as const] : []),
    "separator",
    "exportContainer",
    "exportImage",
  ];
}

export const composeCommands = {
  up: "up -d",
  stop: "stop",
  down: "down",
  buildUp: "up -d --build",
  build: "build",
  restart: "restart",
} as const;

export type ComposeCommand = (typeof composeCommands)[keyof typeof composeCommands];
export function resolveComposeMenuAction(
  action: ComposeMenuAction,
): { kind: "run"; command: ComposeCommand } | { kind: "export"; target: "container" | "image" } {
  if (action === "exportContainer" || action === "exportImage") {
    return { kind: "export", target: action === "exportContainer" ? "container" : "image" };
  }
  return { kind: "run", command: composeCommands[action] };
}
