export type JavaBuildTool = "maven" | "gradle";

/** 「更多操作」下拉里的一条常用命令(maven/gradle 生命周期目标) */
export interface JavaCommandAction {
  /** 前端 i18n 键(java.clean / java.package / ...) */
  key: string;
  command: string;
}

/** 一个 Spring Boot 构建文件的运行分组(monorepo 下可能有多个) */
export interface JavaBuildGroup {
  /** 构建文件所在目录的相对路径('/' 分隔),根目录为 "." */
  dir: string;
  tool: JavaBuildTool;
  /** 运行命令应执行的工作目录的相对路径(多模块工程统一在项目根执行) */
  run_dir: string;
  /** 平台相关的运行命令(优先项目内 wrapper,否则用 PATH 上的 mvn/gradle) */
  run_command: string;
  /** 常用操作(clean/package/install/test 等),与 run_command 同一执行目录 */
  more_actions: JavaCommandAction[];
}

/** 用户在设置页登记的 JDK(开发环境配置) */
export interface JdkConfig {
  id: string;
  name: string;
  /** JDK 根目录(JAVA_HOME) */
  path: string;
}

/** 自动探测到的 JDK 候选(detect_jdks);install_jdk 安装成功也返回同一结构 */
export interface JdkCandidate {
  path: string;
  /** `java -version` 解析出的版本串,如 "17.0.2" / "1.8.0_392" */
  version: string;
}

/** 在线安装源(list_remote_jdks / install_jdk 的 vendor 参数) */
export type JdkVendor = "adoptium" | "zulu";

/** 某安装源可在线安装的 JDK 大版本(list_remote_jdks) */
export interface RemoteJdkRelease {
  /** 主版本号(8 / 11 / 17 / 21 / 25 ...) */
  major: number;
  /** 该主版本当前最新的完整版本串,如 "17.0.20+8" */
  version: string;
}

/** 工具链所属生态(detect_toolchains 输出的分组) */
