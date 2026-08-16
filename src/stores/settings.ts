import { ref } from "vue";
import { defineStore } from "pinia";
import { emit } from "@tauri-apps/api/event";
import { load, type Store } from "@tauri-apps/plugin-store";
import { homeDir, join } from "@tauri-apps/api/path";
import { setI18nLocale, type SupportedLocale } from "@/i18n";
import { isOpenWithId, normalizeOpenWithOrder } from "@/lib/open-with";
import type { CustomOpenWith, JdkConfig, OpenWithId } from "@/types";

/** 主题相关设置变更的跨窗口广播:通知托盘弹窗等其它窗口同步重渲自身 DOM */
const THEME_CHANGED_EVENT = "settings://theme-changed";
/** 打开方式(排序 + 默认项)变更的跨窗口广播:各 webview 的 Pinia store 与 localStorage 读写互不可见 */
const OPEN_WITH_CHANGED_EVENT = "settings://open-with-changed";

export type ThemeMode = "system" | "light" | "dark";
export type ThemeSkin = "default" | "island" | "glass";
export type MdTheme = "default" | "github" | "notion" | "serif";
export type Language = SupportedLocale;
export type ProjectsViewMode = "grid" | "table";
export type ProjectsSortKey = "name" | "updated" | "created";
/** 关闭主窗口行为:tray = 最小化到系统托盘,exit = 直接退出 */
export type CloseAction = "tray" | "exit";

// 应用数据统一存放于用户主目录下的 .repomeow 目录(与 Rust 端 APP_DATA_DIR_NAME 保持一致)
const APP_DATA_DIR_NAME = ".repomeow";
const STORE_FILE = "settings.json";
/** 打开方式列表顺序的 localStorage 键(纯 UI 偏好,不进 settings.json) */
const OPEN_WITH_ORDER_CACHE_KEY = "repomeow:open-with-order";
/** JDK 配置三键的 localStorage 键(开发环境偏好,不进 settings.json;settings.json 里的历史数据不迁移) */
const JDK_LIST_CACHE_KEY = "repomeow:jdk-list";
const DEFAULT_JDK_CACHE_KEY = "repomeow:default-jdk-id";
const PROJECT_JDK_MAP_CACHE_KEY = "repomeow:project-jdk-map";

/** 从 localStorage 读 JSON;缺失/不可用/非法时返回 undefined,由调用方回退默认值 */
function readLocalJson(key: string): unknown {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as unknown) : undefined;
  } catch {
    return undefined;
  }
}

/** 把值序列化写入 localStorage;不可用时静默降级(本次会话内仍生效,重启后回退默认) */
function persistLocalJson(key: string, value: unknown) {
  try {
    window.localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* localStorage 不可用时静默降级 */
  }
}

// AI 接入参数(OpenAI Chat Completions 兼容):baseUrl/apiKey/model 均无默认值,
// 由用户在设置页填写;任一缺失时调用方需先校验。

export const useSettingsStore = defineStore("settings", () => {
  const theme = ref<ThemeMode>("system");
  const themeSkin = ref<ThemeSkin>("default");
  const mdTheme = ref<MdTheme>("default");
  const language = ref<Language>("zh-CN");
  /** 用户配置的外部打开方式。 */
  const customOpenWith = ref<CustomOpenWith[]>([]);
  const defaultOpenWith = ref<OpenWithId>("explorer");
  /** 打开方式列表顺序(设置页可拖拽调整,下拉菜单同步遵循) */
  const openWithOrder = ref<OpenWithId[]>(normalizeOpenWithOrder([], customOpenWith.value));
  const aiBaseUrl = ref("");
  const aiApiKey = ref("");
  const aiModel = ref("");
  /** AI 调用并发上限(1-5),适用于批量生成报告等所有 AI 请求场景 */
  const aiConcurrency = ref(2);
  /** 项目列表视图模式(grid / table) */
  const projectsViewMode = ref<ProjectsViewMode>("grid");
  /** 项目列表排序方式 */
  const projectsSortKey = ref<ProjectsSortKey>("name");
  /** 启动时自动检查更新 */
  const autoCheckUpdate = ref(true);
  /** 关闭主窗口行为(默认最小化到托盘) */
  const closeAction = ref<CloseAction>("tray");
  /** 启用 GitHub CLI(gh)作为「账号仓库」的虚拟账号来源(默认关闭,opt-in) */
  const enableGhCli = ref(false);
  /** 新建 worktree 的默认目录模板:支持 {branch} 占位符与相对路径(相对主工作区根解析) */
  const worktreeDirTemplate = ref(".worktrees/{branch}");
  /** 用户登记的 JDK 列表(开发环境配置,Spring Boot 运行按项目选用) */
  const jdkList = ref<JdkConfig[]>([]);
  /** 默认 JDK 的 id(项目未单独选择时使用;空 = 不注入,走系统 PATH) */
  const defaultJdkId = ref("");
  /** 按项目选择的 JDK(projectId -> jdk id;缺省 = 跟随默认 JDK) */
  const projectJdkMap = ref<Record<string, string>>({});

  let fileStore: Store | null = null;
  let initialized = false;

  const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

  // 将主题相关键镜像到 localStorage,供 index.html 内联脚本在首帧绘制前同步读取,
  // 避免异步加载 settings.json 期间的主题闪烁。权威来源仍是 tauri-plugin-store。
  function syncThemeCache() {
    try {
      window.localStorage.setItem(
        "repomeow:theme-cache",
        JSON.stringify({
          theme: theme.value,
          themeSkin: themeSkin.value,
          mdTheme: mdTheme.value,
        }),
      );
    } catch {
      /* localStorage 不可用时静默降级:首屏可能仍闪烁一次,不影响功能 */
    }
  }

  function applyTheme() {
    const dark = theme.value === "dark" || (theme.value === "system" && systemDark.matches);
    const root = document.documentElement;
    root.classList.toggle("dark", dark);
    if (themeSkin.value === "default") {
      root.removeAttribute("data-theme");
    } else {
      root.setAttribute("data-theme", themeSkin.value);
    }
    syncThemeCache();
  }

  function applyMdTheme() {
    const root = document.documentElement;
    if (mdTheme.value === "default") {
      root.removeAttribute("data-md-theme");
    } else {
      root.setAttribute("data-md-theme", mdTheme.value);
    }
    syncThemeCache();
  }

  function onSystemThemeChange() {
    if (theme.value === "system") applyTheme();
  }

  /**
   * 同步其他窗口广播过来的最新主题值,覆盖本地 ref 后重新 apply 到 DOM。
   * 用于托盘弹窗跟随主窗口的主题切换(各 webview 的 Pinia store 互相独立,
   * 仅靠 init 时从 settings.json 读一次,后续主窗口改了不会自动同步)。
   */
  function syncThemeFromExternal(snapshot: { theme: string; themeSkin: string; mdTheme: string }) {
    if (snapshot.theme === "light" || snapshot.theme === "dark" || snapshot.theme === "system") {
      theme.value = snapshot.theme;
    }
    if (
      snapshot.themeSkin === "default" ||
      snapshot.themeSkin === "island" ||
      snapshot.themeSkin === "glass"
    ) {
      themeSkin.value = snapshot.themeSkin;
    }
    if (
      snapshot.mdTheme === "default" ||
      snapshot.mdTheme === "github" ||
      snapshot.mdTheme === "notion" ||
      snapshot.mdTheme === "serif"
    ) {
      mdTheme.value = snapshot.mdTheme;
    }
    applyTheme();
    applyMdTheme();
  }

  function normalizeCustomOpenWith(saved: unknown): CustomOpenWith[] {
    if (typeof saved === "string") {
      try {
        return normalizeCustomOpenWith(JSON.parse(saved));
      } catch {
        return [];
      }
    }
    if (!Array.isArray(saved)) return [];
    const ids = new Set<string>();
    return saved.flatMap((item) => {
      if (
        !item ||
        typeof item !== "object" ||
        !("id" in item) ||
        !("name" in item) ||
        !("command" in item) ||
        !("icon" in item) ||
        typeof item.id !== "string" ||
        typeof item.name !== "string" ||
        typeof item.command !== "string" ||
        typeof item.icon !== "string" ||
        !item.id.trim() ||
        !item.name.trim() ||
        !item.command.trim() ||
        ids.has(item.id)
      ) {
        return [];
      }
      ids.add(item.id);
      return [
        {
          id: item.id,
          name: item.name.trim(),
          command: item.command.trim(),
          icon: item.icon,
        },
      ];
    });
  }

  /** 解析持久化的 JDK 列表:逐字段校验,按 id 与路径去重 */
  function normalizeJdkList(saved: unknown): JdkConfig[] {
    if (typeof saved === "string") {
      try {
        return normalizeJdkList(JSON.parse(saved));
      } catch {
        return [];
      }
    }
    if (!Array.isArray(saved)) return [];
    const ids = new Set<string>();
    const paths = new Set<string>();
    return saved.flatMap((item) => {
      if (
        !item ||
        typeof item !== "object" ||
        !("id" in item) ||
        !("name" in item) ||
        !("path" in item) ||
        typeof item.id !== "string" ||
        typeof item.name !== "string" ||
        typeof item.path !== "string" ||
        !item.id.trim() ||
        !item.name.trim() ||
        !item.path.trim() ||
        ids.has(item.id) ||
        paths.has(item.path)
      ) {
        return [];
      }
      ids.add(item.id);
      paths.add(item.path);
      return [{ id: item.id, name: item.name.trim(), path: item.path.trim() }];
    });
  }

  /** 解析持久化的按项目 JDK 选择:仅保留 projectId -> jdkId 的字符串映射 */
  function normalizeProjectJdkMap(saved: unknown): Record<string, string> {
    if (typeof saved === "string") {
      try {
        return normalizeProjectJdkMap(JSON.parse(saved));
      } catch {
        return {};
      }
    }
    if (!saved || typeof saved !== "object" || Array.isArray(saved)) return {};
    return Object.fromEntries(
      Object.entries(saved as Record<string, unknown>).flatMap(([k, v]) =>
        typeof v === "string" && v.trim() ? [[k, v] as const] : [],
      ),
    );
  }

  /** 把当前打开方式顺序写入 localStorage(设置页拖拽排序与外部同步共用) */
  function persistOpenWithOrderCache() {
    try {
      window.localStorage.setItem(OPEN_WITH_ORDER_CACHE_KEY, JSON.stringify(openWithOrder.value));
    } catch {
      /* localStorage 不可用时静默降级:本次会话内顺序仍生效 */
    }
  }

  /** 从 localStorage 重读打开方式顺序(init 与托盘弹窗刷新兜底共用) */
  function reloadOpenWithOrderCache() {
    try {
      const raw = window.localStorage.getItem(OPEN_WITH_ORDER_CACHE_KEY);
      if (raw) {
        const parsed: unknown = JSON.parse(raw);
        openWithOrder.value = normalizeOpenWithOrder(
          Array.isArray(parsed) ? parsed : [],
          customOpenWith.value,
        );
      }
    } catch {
      /* localStorage 不可用或 JSON 非法时保持当前顺序 */
    }
  }

  /**
   * 同步其他窗口广播过来的打开方式快照(自定义项 + 排序 + 默认项),覆盖本地 ref 并镜像到 localStorage。
   * 各 webview 的 Pinia store 互相独立,浏览器 storage 事件也不跨 Tauri 窗口派发,
   * 主窗口在设置页调整后,托盘弹窗只能靠这条广播保持一致。
   */
  function syncOpenWithFromExternal(snapshot: {
    customOpenWith: unknown;
    order: unknown;
    defaultOpenWith: unknown;
  }) {
    customOpenWith.value = normalizeCustomOpenWith(snapshot.customOpenWith);
    if (Array.isArray(snapshot.order)) {
      openWithOrder.value = normalizeOpenWithOrder(snapshot.order, customOpenWith.value);
      persistOpenWithOrderCache();
    }
    if (isOpenWithId(snapshot.defaultOpenWith, customOpenWith.value)) {
      defaultOpenWith.value = snapshot.defaultOpenWith;
    } else {
      defaultOpenWith.value = "explorer";
    }
  }

  /**
   * 从持久层重读打开方式设置(localStorage 顺序 + settings.json 默认项)。
   * 托盘弹窗每次显示时调用,兜底广播注册前错过的变更。
   */
  async function reloadOpenWith() {
    const savedCustom = await fileStore?.get<unknown>("customOpenWith");
    customOpenWith.value = normalizeCustomOpenWith(savedCustom);
    reloadOpenWithOrderCache();
    const saved = await fileStore?.get<string>("defaultOpenWith");
    if (isOpenWithId(saved, customOpenWith.value)) {
      defaultOpenWith.value = saved;
    } else {
      defaultOpenWith.value = "explorer";
    }
  }

  /**
   * 从 localStorage 重读 JDK 配置三键,托盘弹窗每次显示时的兜底:
   * localStorage 数据跨 webview 共享但 storage 事件不跨 Tauri 窗口派发,
   * 主窗口改过 JDK 后,托盘 webview 内的 store 实例需在此补读才能解析到最新 JAVA_HOME。
   */
  function reloadJdkConfig() {
    jdkList.value = normalizeJdkList(readLocalJson(JDK_LIST_CACHE_KEY));
    const savedDefault = readLocalJson(DEFAULT_JDK_CACHE_KEY);
    defaultJdkId.value =
      typeof savedDefault === "string" && jdkList.value.some((j) => j.id === savedDefault)
        ? savedDefault
        : "";
    projectJdkMap.value = normalizeProjectJdkMap(readLocalJson(PROJECT_JDK_MAP_CACHE_KEY));
  }

  // ── lifecycle ─────────────────────────────────────────────

  async function init() {
    if (initialized) return;
    initialized = true;

    fileStore = await load(await join(await homeDir(), APP_DATA_DIR_NAME, STORE_FILE), {
      defaults: {
        theme: "system",
        themeSkin: "default",
        mdTheme: "default",
        language: "zh-CN",
        defaultOpenWith: "explorer",
        customOpenWith: "[]",
        aiBaseUrl: "",
        aiApiKey: "",
        aiModel: "",
        aiConcurrency: "2",
        projectsViewMode: "grid",
        projectsSortKey: "name",
        autoCheckUpdate: "true",
        closeAction: "tray",
        enableGhCli: "false",
        worktreeDirTemplate: ".worktrees/{branch}",
      },
    });
    const savedTheme = await fileStore.get<ThemeMode>("theme");
    if (savedTheme === "light" || savedTheme === "dark" || savedTheme === "system") {
      theme.value = savedTheme;
    }
    const savedSkin = await fileStore.get<ThemeSkin>("themeSkin");
    if (savedSkin === "default" || savedSkin === "island" || savedSkin === "glass") {
      themeSkin.value = savedSkin;
    }
    const savedMdTheme = await fileStore.get<MdTheme>("mdTheme");
    if (
      savedMdTheme === "default" ||
      savedMdTheme === "github" ||
      savedMdTheme === "notion" ||
      savedMdTheme === "serif"
    ) {
      mdTheme.value = savedMdTheme;
    }
    const savedLanguage = await fileStore.get<Language>("language");
    if (savedLanguage === "zh-CN" || savedLanguage === "en-US") {
      language.value = savedLanguage;
      setI18nLocale(savedLanguage);
    }
    const savedCustomOpenWith = await fileStore.get<unknown>("customOpenWith");
    customOpenWith.value = normalizeCustomOpenWith(savedCustomOpenWith);
    const savedOpenWith = await fileStore.get<string>("defaultOpenWith");
    if (isOpenWithId(savedOpenWith, customOpenWith.value)) {
      defaultOpenWith.value = savedOpenWith;
    }
    // 打开方式排序:localStorage 里的 JSON 数组,过滤已删除项并补齐新增项,非法值回退默认顺序
    reloadOpenWithOrderCache();
    // AI 配置为自由文本:trim 后非空才赋值,空值保持初始空(无默认值可回退)
    const savedAiBaseUrl = await fileStore.get<string>("aiBaseUrl");
    if (typeof savedAiBaseUrl === "string" && savedAiBaseUrl.trim()) {
      aiBaseUrl.value = savedAiBaseUrl.trim();
    }
    const savedAiApiKey = await fileStore.get<string>("aiApiKey");
    if (typeof savedAiApiKey === "string") {
      aiApiKey.value = savedAiApiKey;
    }
    const savedAiModel = await fileStore.get<string>("aiModel");
    if (typeof savedAiModel === "string" && savedAiModel.trim()) {
      aiModel.value = savedAiModel.trim();
    }
    // 并发上限存为字符串,解析后限制在 1-5,非法值回退默认 2
    const savedConcurrency = await fileStore.get<string>("aiConcurrency");
    if (typeof savedConcurrency === "string") {
      const n = Number.parseInt(savedConcurrency, 10);
      if (Number.isFinite(n)) {
        aiConcurrency.value = Math.min(5, Math.max(1, n));
      }
    }
    // 视图模式:白名单校验,非法值回退 grid
    const savedViewMode = await fileStore.get<ProjectsViewMode>("projectsViewMode");
    if (savedViewMode === "grid" || savedViewMode === "table") {
      projectsViewMode.value = savedViewMode;
    }
    // 排序键:白名单校验,非法值回退 name
    const savedSortKey = await fileStore.get<ProjectsSortKey>("projectsSortKey");
    if (savedSortKey === "name" || savedSortKey === "updated" || savedSortKey === "created") {
      projectsSortKey.value = savedSortKey;
    }
    // 自动检查更新:存为字符串 "true"/"false",非法值回退 true
    const savedAutoCheckUpdate = await fileStore.get<string>("autoCheckUpdate");
    if (savedAutoCheckUpdate === "true" || savedAutoCheckUpdate === "false") {
      autoCheckUpdate.value = savedAutoCheckUpdate === "true";
    }
    // 关闭行为:白名单校验,非法值回退 tray(该键同时被 Rust 侧 on_window_event 读取)
    const savedCloseAction = await fileStore.get<CloseAction>("closeAction");
    if (savedCloseAction === "tray" || savedCloseAction === "exit") {
      closeAction.value = savedCloseAction;
    }
    // GitHub CLI 集成开关:存为字符串 "true"/"false",非法值回退 false
    const savedEnableGhCli = await fileStore.get<string>("enableGhCli");
    if (savedEnableGhCli === "true" || savedEnableGhCli === "false") {
      enableGhCli.value = savedEnableGhCli === "true";
    }
    // worktree 默认目录模板:自由文本,trim 后非空才采用
    const savedWorktreeDir = await fileStore.get<string>("worktreeDirTemplate");
    if (typeof savedWorktreeDir === "string" && savedWorktreeDir.trim()) {
      worktreeDirTemplate.value = savedWorktreeDir.trim();
    }
    // JDK 配置存 localStorage,不进 settings.json(历史 settings.json 数据不迁移);
    // 默认项引用了不存在的 id 时回退空
    jdkList.value = normalizeJdkList(readLocalJson(JDK_LIST_CACHE_KEY));
    const savedDefaultJdk = readLocalJson(DEFAULT_JDK_CACHE_KEY);
    if (
      typeof savedDefaultJdk === "string" &&
      jdkList.value.some((j) => j.id === savedDefaultJdk)
    ) {
      defaultJdkId.value = savedDefaultJdk;
    }
    projectJdkMap.value = normalizeProjectJdkMap(readLocalJson(PROJECT_JDK_MAP_CACHE_KEY));
    applyTheme();
    applyMdTheme();
    systemDark.addEventListener("change", onSystemThemeChange);
  }

  async function persist(key: string, value: string) {
    if (!fileStore) return;
    await fileStore.set(key, value);
    await fileStore.save();
  }

  async function setTheme(value: ThemeMode) {
    theme.value = value;
    applyTheme();
    await persist("theme", value);
    // 带全量主题快照:接收方(托盘弹窗等独立 webview 窗口)的 store 是另一实例,
    // 不会自动同步这边的 ref,必须把最新三项一起发过去,接收方覆盖本地 ref 后再 apply
    await emit(THEME_CHANGED_EVENT, {
      theme: theme.value,
      themeSkin: themeSkin.value,
      mdTheme: mdTheme.value,
    });
  }

  async function setThemeSkin(value: ThemeSkin) {
    themeSkin.value = value;
    applyTheme();
    await persist("themeSkin", value);
    await emit(THEME_CHANGED_EVENT, {
      theme: theme.value,
      themeSkin: themeSkin.value,
      mdTheme: mdTheme.value,
    });
  }

  async function setMdTheme(value: MdTheme) {
    mdTheme.value = value;
    applyMdTheme();
    await persist("mdTheme", value);
    await emit(THEME_CHANGED_EVENT, {
      theme: theme.value,
      themeSkin: themeSkin.value,
      mdTheme: mdTheme.value,
    });
  }

  async function setLanguage(value: Language) {
    language.value = value;
    setI18nLocale(value);
    await persist("language", value);
  }

  /** 打开方式变更广播:带全量快照(自定义项 + 排序 + 默认项),接收方无需再读持久层 */
  async function emitOpenWithChanged() {
    await emit(OPEN_WITH_CHANGED_EVENT, {
      customOpenWith: customOpenWith.value,
      order: openWithOrder.value,
      defaultOpenWith: defaultOpenWith.value,
    });
  }

  async function setDefaultOpenWith(value: OpenWithId) {
    if (!isOpenWithId(value, customOpenWith.value)) return;
    defaultOpenWith.value = value;
    await persist("defaultOpenWith", value);
    await emitOpenWithChanged();
  }

  async function setOpenWithOrder(value: OpenWithId[]) {
    openWithOrder.value = normalizeOpenWithOrder(value, customOpenWith.value);
    persistOpenWithOrderCache();
    await emitOpenWithChanged();
  }

  async function saveCustomOpenWith(value: CustomOpenWith) {
    const next = {
      id: value.id,
      name: value.name.trim(),
      command: value.command.trim(),
      icon: value.icon,
    };
    if (!next.id || !next.name || !next.command) return;
    const existing = customOpenWith.value.findIndex((option) => option.id === next.id);
    if (existing === -1) {
      customOpenWith.value = [...customOpenWith.value, next];
    } else {
      customOpenWith.value = customOpenWith.value.map((option, index) =>
        index === existing ? next : option,
      );
    }
    openWithOrder.value = normalizeOpenWithOrder(openWithOrder.value, customOpenWith.value);
    persistOpenWithOrderCache();
    await persist("customOpenWith", JSON.stringify(customOpenWith.value));
    await emitOpenWithChanged();
  }

  async function removeCustomOpenWith(id: string) {
    if (!customOpenWith.value.some((option) => option.id === id)) return;
    const customId = `custom:${id}` as const;
    customOpenWith.value = customOpenWith.value.filter((option) => option.id !== id);
    openWithOrder.value = normalizeOpenWithOrder(openWithOrder.value, customOpenWith.value);
    if (defaultOpenWith.value === customId) {
      defaultOpenWith.value = "explorer";
      await persist("defaultOpenWith", defaultOpenWith.value);
    }
    persistOpenWithOrderCache();
    await persist("customOpenWith", JSON.stringify(customOpenWith.value));
    await emitOpenWithChanged();
  }

  async function setAiBaseUrl(value: string) {
    aiBaseUrl.value = value.trim();
    await persist("aiBaseUrl", aiBaseUrl.value);
  }

  async function setAiApiKey(value: string) {
    aiApiKey.value = value.trim();
    await persist("aiApiKey", aiApiKey.value);
  }

  async function setAiModel(value: string) {
    aiModel.value = value.trim();
    await persist("aiModel", aiModel.value);
  }

  async function setAiConcurrency(value: number) {
    const n = Math.min(5, Math.max(1, Math.round(value)));
    aiConcurrency.value = n;
    await persist("aiConcurrency", String(n));
  }

  async function setProjectsViewMode(value: ProjectsViewMode) {
    if (value !== "grid" && value !== "table") return;
    projectsViewMode.value = value;
    await persist("projectsViewMode", value);
  }

  async function setProjectsSortKey(value: ProjectsSortKey) {
    if (value !== "name" && value !== "updated" && value !== "created") return;
    projectsSortKey.value = value;
    await persist("projectsSortKey", value);
  }

  async function setAutoCheckUpdate(value: boolean) {
    autoCheckUpdate.value = value;
    await persist("autoCheckUpdate", String(value));
  }

  async function setCloseAction(value: CloseAction) {
    if (value !== "tray" && value !== "exit") return;
    closeAction.value = value;
    await persist("closeAction", value);
  }

  async function setEnableGhCli(value: boolean) {
    enableGhCli.value = value;
    await persist("enableGhCli", String(value));
  }

  async function setWorktreeDirTemplate(value: string) {
    const v = value.trim();
    if (!v) return;
    worktreeDirTemplate.value = v;
    await persist("worktreeDirTemplate", v);
  }

  // ── 开发环境(JDK) ────────────────────────────────────────────

  /** 把 JDK 配置三键统一写入 localStorage(任一键变更后调用) */
  function persistJdkCache() {
    persistLocalJson(JDK_LIST_CACHE_KEY, jdkList.value);
    persistLocalJson(DEFAULT_JDK_CACHE_KEY, defaultJdkId.value);
    persistLocalJson(PROJECT_JDK_MAP_CACHE_KEY, projectJdkMap.value);
  }

  /** 新增或更新一个 JDK(按 id upsert,路径去重);首个条目自动成为默认 */
  async function saveJdk(value: JdkConfig) {
    const next = { id: value.id, name: value.name.trim(), path: value.path.trim() };
    if (!next.id || !next.name || !next.path) return;
    if (jdkList.value.some((j) => j.path === next.path && j.id !== next.id)) return;
    const existing = jdkList.value.findIndex((j) => j.id === next.id);
    if (existing === -1) {
      jdkList.value = [...jdkList.value, next];
      if (!defaultJdkId.value) {
        defaultJdkId.value = next.id;
      }
    } else {
      jdkList.value = jdkList.value.map((j, i) => (i === existing ? next : j));
    }
    persistJdkCache();
  }

  /** 批量追加 JDK(自动探测用):过滤已存在路径后追加,单次落库;返回实际新增数 */
  async function addJdks(jdks: JdkConfig[]) {
    const known = new Set(jdkList.value.map((j) => j.path.toLowerCase()));
    const additions: JdkConfig[] = [];
    for (const jdk of jdks) {
      const next = { id: jdk.id, name: jdk.name.trim(), path: jdk.path.trim() };
      const key = next.path.toLowerCase();
      if (!next.id || !next.name || !next.path || known.has(key)) continue;
      known.add(key);
      additions.push(next);
    }
    if (!additions.length) return 0;
    jdkList.value = [...jdkList.value, ...additions];
    if (!defaultJdkId.value) {
      defaultJdkId.value = additions[0].id;
    }
    persistJdkCache();
    return additions.length;
  }

  /** 删除 JDK:若为默认项则回退到首个剩余项;同时清理引用它的项目选择 */
  async function removeJdk(id: string) {
    if (!jdkList.value.some((j) => j.id === id)) return;
    jdkList.value = jdkList.value.filter((j) => j.id !== id);
    if (defaultJdkId.value === id) {
      defaultJdkId.value = jdkList.value[0]?.id ?? "";
    }
    if (Object.values(projectJdkMap.value).includes(id)) {
      projectJdkMap.value = Object.fromEntries(
        Object.entries(projectJdkMap.value).filter(([, v]) => v !== id),
      );
    }
    persistJdkCache();
  }

  async function setDefaultJdk(id: string) {
    if (!jdkList.value.some((j) => j.id === id)) return;
    defaultJdkId.value = id;
    persistJdkCache();
  }

  /** 设置/清除按项目选择的 JDK(jdkId 为空 = 跟随默认 JDK);projectId 接受项目 id(number) */
  async function setProjectJdk(projectId: string | number, jdkId: string) {
    const key = String(projectId);
    if (!key) return;
    if (jdkId) {
      if (!jdkList.value.some((j) => j.id === jdkId)) return;
      projectJdkMap.value = { ...projectJdkMap.value, [key]: jdkId };
    } else {
      const rest = { ...projectJdkMap.value };
      delete rest[key];
      projectJdkMap.value = rest;
    }
    persistJdkCache();
  }

  return {
    theme,
    themeSkin,
    mdTheme,
    language,
    customOpenWith,
    defaultOpenWith,
    openWithOrder,
    aiBaseUrl,
    aiApiKey,
    aiModel,
    aiConcurrency,
    projectsViewMode,
    projectsSortKey,
    autoCheckUpdate,
    closeAction,
    enableGhCli,
    worktreeDirTemplate,
    jdkList,
    defaultJdkId,
    projectJdkMap,
    init,
    applyTheme,
    applyMdTheme,
    syncThemeFromExternal,
    syncOpenWithFromExternal,
    reloadOpenWith,
    reloadJdkConfig,
    setTheme,
    setThemeSkin,
    setMdTheme,
    setLanguage,
    setDefaultOpenWith,
    setOpenWithOrder,
    saveCustomOpenWith,
    removeCustomOpenWith,
    setAiBaseUrl,
    setAiApiKey,
    setAiModel,
    setAiConcurrency,
    setProjectsViewMode,
    setProjectsSortKey,
    setAutoCheckUpdate,
    setCloseAction,
    setEnableGhCli,
    setWorktreeDirTemplate,
    saveJdk,
    addJdks,
    removeJdk,
    setDefaultJdk,
    setProjectJdk,
  };
});
