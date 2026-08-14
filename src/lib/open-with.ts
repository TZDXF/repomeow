import type { Component } from "vue";
import {
  Braces,
  Code,
  CodeXml,
  Coffee,
  Cog,
  Cpu,
  FolderOpen,
  Globe,
  Rocket,
  Sparkles,
  SquareCode,
  Terminal,
  Wind,
  Worm,
  Zap,
} from "@lucide/vue";
import { commandIcon } from "@/lib/command-icons";
import { cmd } from "@/lib/tauri";
import type { CustomOpenWith, EditorKind, OpenWithId } from "@/types";

/** 打开方式选项元数据 —— 三处共享(OpenWithMenu 下拉 / OpenWithSettings / ProjectActionsMenu) */
export interface BuiltInOpenWithOption {
  id: EditorKind;
  kind: EditorKind;
  icon: Component;
  labelKey: string;
  descKey: string;
  custom: false;
}

export interface CustomOpenWithOption {
  id: `custom:${string}`;
  name: string;
  command: string;
  icon: Component;
  custom: true;
}

export type OpenWithOption = BuiltInOpenWithOption | CustomOpenWithOption;

export const OPEN_WITH_OPTIONS: readonly BuiltInOpenWithOption[] = [
  {
    id: "explorer",
    kind: "explorer",
    icon: FolderOpen,
    labelKey: "openWith.explorer",
    descKey: "openWith.openInExplorer",
    custom: false,
  },
  {
    id: "vscode",
    kind: "vscode",
    icon: Code,
    labelKey: "openWith.vscode",
    descKey: "openWith.openInVscode",
    custom: false,
  },
  {
    id: "cursor",
    kind: "cursor",
    icon: SquareCode,
    labelKey: "openWith.cursor",
    descKey: "openWith.openInCursor",
    custom: false,
  },
  {
    id: "windsurf",
    kind: "windsurf",
    icon: Wind,
    labelKey: "openWith.windsurf",
    descKey: "openWith.openInWindsurf",
    custom: false,
  },
  {
    id: "trae",
    kind: "trae",
    icon: Sparkles,
    labelKey: "openWith.trae",
    descKey: "openWith.openInTrae",
    custom: false,
  },
  {
    id: "vscodium",
    kind: "vscodium",
    icon: CodeXml,
    labelKey: "openWith.vscodium",
    descKey: "openWith.openInVscodium",
    custom: false,
  },
  {
    id: "zed",
    kind: "zed",
    icon: Zap,
    labelKey: "openWith.zed",
    descKey: "openWith.openInZed",
    custom: false,
  },
  {
    id: "sublime",
    kind: "sublime",
    icon: Braces,
    labelKey: "openWith.sublime",
    descKey: "openWith.openInSublime",
    custom: false,
  },
  {
    id: "idea",
    kind: "idea",
    icon: Coffee,
    labelKey: "openWith.idea",
    descKey: "openWith.openInIdea",
    custom: false,
  },
  {
    id: "webstorm",
    kind: "webstorm",
    icon: Globe,
    labelKey: "openWith.webstorm",
    descKey: "openWith.openInWebstorm",
    custom: false,
  },
  {
    id: "goland",
    kind: "goland",
    icon: Rocket,
    labelKey: "openWith.goland",
    descKey: "openWith.openInGoland",
    custom: false,
  },
  {
    id: "pycharm",
    kind: "pycharm",
    icon: Worm,
    labelKey: "openWith.pycharm",
    descKey: "openWith.openInPycharm",
    custom: false,
  },
  {
    id: "clion",
    kind: "clion",
    icon: Cpu,
    labelKey: "openWith.clion",
    descKey: "openWith.openInClion",
    custom: false,
  },
  {
    id: "rustrover",
    kind: "rustrover",
    icon: Cog,
    labelKey: "openWith.rustrover",
    descKey: "openWith.openInRustrover",
    custom: false,
  },
  {
    id: "terminal",
    kind: "terminal",
    icon: Terminal,
    labelKey: "openWith.terminal",
    descKey: "openWith.openInTerminal",
    custom: false,
  },
] as const;

export function customOpenWithId(id: string): `custom:${string}` {
  return `custom:${id}`;
}

export function isCustomOpenWithId(id: string): id is `custom:${string}` {
  return id.startsWith("custom:");
}

export function isOpenWithId(
  value: unknown,
  customOpenWith: readonly CustomOpenWith[],
): value is OpenWithId {
  return (
    OPEN_WITH_OPTIONS.some((option) => option.id === value) ||
    (typeof value === "string" &&
      isCustomOpenWithId(value) &&
      customOpenWith.some((option) => customOpenWithId(option.id) === value))
  );
}

export function openWithOptions(customOpenWith: readonly CustomOpenWith[]): OpenWithOption[] {
  return [
    ...OPEN_WITH_OPTIONS,
    ...customOpenWith.map((option) => ({
      id: customOpenWithId(option.id),
      name: option.name,
      command: option.command,
      icon: commandIcon(option.icon) ?? Terminal,
      custom: true as const,
    })),
  ];
}

/**
 * 归一化打开方式顺序:过滤无效项,末尾按默认顺序补齐新增项,保证始终覆盖全部选项。
 * 自定义项被删除后会自动从旧排序中剔除。
 */
export function normalizeOpenWithOrder(
  saved: readonly unknown[],
  customOpenWith: readonly CustomOpenWith[] = [],
): OpenWithId[] {
  const all = openWithOptions(customOpenWith).map((option) => option.id);
  const kept = saved.filter((id): id is OpenWithId => all.includes(id as OpenWithId));
  return [...kept, ...all.filter((id) => !kept.includes(id))];
}

/** 按用户自定义顺序排序打开方式选项(设置页拖拽排序的结果,三处共享) */
export function sortOpenWithOptions(
  order: readonly OpenWithId[],
  customOpenWith: readonly CustomOpenWith[] = [],
): OpenWithOption[] {
  const rank = new Map(order.map((id, index) => [id, index]));
  return [...openWithOptions(customOpenWith)].sort(
    (a, b) =>
      (rank.get(a.id) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.id) ?? Number.MAX_SAFE_INTEGER),
  );
}

/** 编辑器可用性:kind → 是否已安装(CLI 在 PATH 中);不含 explorer / terminal */
export type EditorAvailability = Partial<Record<EditorKind, boolean>>;

let availabilityPromise: Promise<EditorAvailability> | null = null;

/** 探测所有命令类编辑器可用性(后端探测一次并持久缓存,前端模块级只请求一次) */
export function getEditorAvailability(): Promise<EditorAvailability> {
  availabilityPromise ??= cmd<EditorAvailability>("detect_editors").catch(
    () => ({}) satisfies EditorAvailability,
  );
  return availabilityPromise;
}

/** 编辑器真实图标:kind → 本机 PNG 缓存文件绝对路径(提取失败无该键或为 null) */
export type EditorIconMap = Record<string, string | null>;

let iconsPromise: Promise<EditorIconMap> | null = null;

/** 取编辑器真实图标(后端从本机 exe / .app 提取并缓存;失败返回空表,前端回退 lucide 图标) */
export function getEditorIcons(): Promise<EditorIconMap> {
  iconsPromise ??= cmd<EditorIconMap>("get_editor_icons").catch(() => ({}));
  return iconsPromise;
}

/** 以某种方式打开项目目录。 */
export function openProjectWith(option: OpenWithOption, path: string): Promise<void> {
  if (option.custom) {
    return cmd("open_with_custom_command", { path, command: option.command, line: null });
  }
  return cmd("open_with", { path, kind: option.kind });
}

/** 以某种方式打开文件或目录,可选传入行号。 */
export function openPathWith(
  option: OpenWithOption,
  path: string,
  line: number | null = null,
): Promise<void> {
  if (option.custom) {
    return cmd("open_with_custom_command", { path, command: option.command, line });
  }
  return cmd("open_in_editor", { path, kind: option.kind, line });
}

/** 平台内置方式,无需探测 */
const ALWAYS_AVAILABLE: ReadonlySet<EditorKind> = new Set(["explorer", "terminal"]);

/**
 * 某内置打开方式是否不可用(命令不在 PATH)。自定义命令由用户负责可执行性,始终展示。
 * 探测结果缺失或不含该 kind 时视为可用,避免偶发探测失败导致永久误禁用。
 */
export function isEditorUnavailable(
  option: OpenWithOption | EditorKind,
  availability: EditorAvailability | null,
): boolean {
  if (typeof option !== "string" && option.custom) {
    return false;
  }
  const kind = typeof option === "string" ? option : option.kind;
  if (ALWAYS_AVAILABLE.has(kind) || availability === null) {
    return false;
  }
  return availability[kind] === false;
}
