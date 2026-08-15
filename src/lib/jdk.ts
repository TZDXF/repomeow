import type { useSettingsStore } from "@/stores/settings";

type SettingsStore = ReturnType<typeof useSettingsStore>;

/** JDK 选择的三种取值(存 projectJdkMap):跟随默认 / 系统 PATH / 显式 jdk id */
export const JDK_FOLLOW_DEFAULT = "__default__";
export const JDK_SYSTEM_PATH = "__path__";

/**
 * 解析项目运行时应注入的 JAVA_HOME:项目单独选择 > 默认 JDK;
 * 显式选了「系统 PATH」或未配置任何 JDK 时返回 undefined(走系统环境)。
 * 详情页 Spring Boot 卡片与托盘弹窗的标记命令执行共用此逻辑。
 */
export function resolveJavaHome(
  store: SettingsStore,
  projectId: number | string,
): string | undefined {
  const mode = store.projectJdkMap[String(projectId)] ?? JDK_FOLLOW_DEFAULT;
  if (mode === JDK_SYSTEM_PATH) return undefined;
  const id = mode === JDK_FOLLOW_DEFAULT ? store.defaultJdkId : mode;
  return store.jdkList.find((j) => j.id === id)?.path;
}
