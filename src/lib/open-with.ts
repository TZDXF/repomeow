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
import { cmd } from "@/lib/tauri";
import type { EditorKind } from "@/types";

/** 打开方式选项元数据 —— 三处共享(OpenWithMenu 下拉 / OpenWithSettings / ProjectActionsMenu) */
export interface OpenWithOption {
  kind: EditorKind;
  icon: Component;
  labelKey: string;
  descKey: string;
}

export const OPEN_WITH_OPTIONS: readonly OpenWithOption[] = [
  {
    kind: "explorer",
    icon: FolderOpen,
    labelKey: "openWith.explorer",
    descKey: "openWith.openInExplorer",
  },
  { kind: "vscode", icon: Code, labelKey: "openWith.vscode", descKey: "openWith.openInVscode" },
  {
    kind: "cursor",
    icon: SquareCode,
    labelKey: "openWith.cursor",
    descKey: "openWith.openInCursor",
  },
  {
    kind: "windsurf",
    icon: Wind,
    labelKey: "openWith.windsurf",
    descKey: "openWith.openInWindsurf",
  },
  { kind: "trae", icon: Sparkles, labelKey: "openWith.trae", descKey: "openWith.openInTrae" },
  {
    kind: "vscodium",
    icon: CodeXml,
    labelKey: "openWith.vscodium",
    descKey: "openWith.openInVscodium",
  },
  { kind: "zed", icon: Zap, labelKey: "openWith.zed", descKey: "openWith.openInZed" },
  {
    kind: "sublime",
    icon: Braces,
    labelKey: "openWith.sublime",
    descKey: "openWith.openInSublime",
  },
  { kind: "idea", icon: Coffee, labelKey: "openWith.idea", descKey: "openWith.openInIdea" },
  {
    kind: "webstorm",
    icon: Globe,
    labelKey: "openWith.webstorm",
    descKey: "openWith.openInWebstorm",
  },
  {
    kind: "goland",
    icon: Rocket,
    labelKey: "openWith.goland",
    descKey: "openWith.openInGoland",
  },
  {
    kind: "pycharm",
    icon: Worm,
    labelKey: "openWith.pycharm",
    descKey: "openWith.openInPycharm",
  },
  { kind: "clion", icon: Cpu, labelKey: "openWith.clion", descKey: "openWith.openInClion" },
  {
    kind: "rustrover",
    icon: Cog,
    labelKey: "openWith.rustrover",
    descKey: "openWith.openInRustrover",
  },
  {
    kind: "terminal",
    icon: Terminal,
    labelKey: "openWith.terminal",
    descKey: "openWith.openInTerminal",
  },
] as const;

/**
 * 归一化打开方式顺序:过滤无效 kind,末尾按默认顺序补齐新增项,保证始终覆盖全部选项。
 * 新增打开方式无需改持久化数据,旧顺序里缺的项会自动排到末尾。
 */
export function normalizeOpenWithOrder(saved: readonly unknown[]): EditorKind[] {
  const all = OPEN_WITH_OPTIONS.map((opt) => opt.kind);
  const kept = saved.filter((k): k is EditorKind => all.includes(k as EditorKind));
  return [...kept, ...all.filter((kind) => !kept.includes(kind))];
}

/** 按用户自定义顺序排序打开方式选项(设置页拖拽排序的结果,三处共享) */
export function sortOpenWithOptions(order: readonly EditorKind[]): OpenWithOption[] {
  const rank = new Map(order.map((kind, i) => [kind, i]));
  return [...OPEN_WITH_OPTIONS].sort(
    (a, b) =>
      (rank.get(a.kind) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.kind) ?? Number.MAX_SAFE_INTEGER),
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

/** 平台内置方式,无需探测 */
const ALWAYS_AVAILABLE: ReadonlySet<EditorKind> = new Set(["explorer", "terminal"]);

/**
 * 某打开方式是否不可用(命令不在 PATH)。
 * 探测结果缺失或不含该 kind 时视为可用,避免偶发探测失败导致永久误禁用。
 */
export function isEditorUnavailable(
  kind: EditorKind,
  availability: EditorAvailability | null,
): boolean {
  if (ALWAYS_AVAILABLE.has(kind) || availability === null) {
    return false;
  }
  return availability[kind] === false;
}
