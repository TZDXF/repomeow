export type EditorKind =
  | "explorer"
  | "vscode"
  | "cursor"
  | "windsurf"
  | "trae"
  | "vscodium"
  | "zed"
  | "sublime"
  | "idea"
  | "webstorm"
  | "goland"
  | "pycharm"
  | "clion"
  | "rustrover"
  | "terminal";

/** 用户在设置中配置的外部打开方式。命令支持 {path} 与 {line} 占位符。 */
export interface CustomOpenWith {
  id: string;
  name: string;
  command: string;
  icon: string;
}

/** 内置打开方式或以 custom: 前缀标识的自定义打开方式。 */
export type OpenWithId = EditorKind | `custom:${string}`;

/** 可隐藏的 UI 项类型:package.json 分组 / 分组内单条命令 / compose 文件 / Spring Boot 构建分组 */
export type HiddenKind = "packageFile" | "packageScript" | "composeFile" | "javaBuild";

/** 项目维度被隐藏的 UI 项(targetKey 含义见各使用处) */
export interface HiddenItem {
  kind: HiddenKind;
  targetKey: string;
}

/** 详情页首屏聚合数据(get_project_overview 一次 IPC 返回) */
