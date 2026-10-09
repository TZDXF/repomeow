import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import type { RunCommandOptions } from "@/lib/terminal";
import type { Project, TerminalSessionInfo } from "@/types";
import { useTerminalStore } from "./terminal";

const mocks = vi.hoisted(() => ({
  onListen: vi.fn(),
  listCommandSessions: vi.fn(),
  runCommandSession: vi.fn(),
  createShellSession: vi.fn(),
  runInTerminal: vi.fn(),
  useSettingsStore: vi.fn(() => ({ embeddedTerminal: false })),
}));

vi.mock("@/lib/tauri", () => ({
  onListen: mocks.onListen,
  runInTerminal: mocks.runInTerminal,
}));

vi.mock("@/stores/settings", () => ({ useSettingsStore: mocks.useSettingsStore }));

vi.mock("@/lib/terminal", () => ({
  TERMINAL_OUTPUT_EVENT: "terminal://output",
  TERMINAL_SESSION_CHANGED_EVENT: "terminal://session-changed",
  appendCapped: vi.fn(),
  createShellSession: mocks.createShellSession,
  getCommandSessionOutput: vi.fn(),
  listCommandSessions: mocks.listCommandSessions,
  removeCommandSession: vi.fn(),
  restartCommandSession: vi.fn(),
  runCommandSession: mocks.runCommandSession,
  stopCommandSession: vi.fn(),
}));

const project: Project = {
  id: 1,
  name: "demo",
  path: "D:/projects/demo",
  description: "",
  tags: [],
  git: null,
  path_exists: true,
  archived_at: null,
  favorited_at: null,
  auto_pull: false,
  wiki_auto_update: false,
  created_at: 0,
  updated_at: 0,
};

const session: TerminalSessionInfo = {
  id: 42,
  project_id: project.id,
  project_name: project.name,
  label: "dev",
  kind: "npm",
  command: "npm run dev",
  interactive: false,
  cwd: project.path,
  status: "running",
  exit_code: null,
  started_at: 1,
  finished_at: null,
};

beforeEach(() => {
  vi.clearAllMocks();
  setActivePinia(createPinia());
  mocks.onListen.mockResolvedValue(() => {});
  mocks.listCommandSessions.mockResolvedValue([]);
  mocks.runCommandSession.mockResolvedValue(session);
  mocks.createShellSession.mockResolvedValue({ ...session, id: 43, interactive: true });
});

describe("内置终端统一执行", () => {
  const commands: { command: string; opts: RunCommandOptions }[] = [
    { command: "npm run dev", opts: { kind: "npm", cwd: "D:/projects/demo/packages/ui" } },
    { command: "echo hello", opts: { kind: "custom", label: "自定义命令" } },
    { command: "docker compose up -d", opts: { kind: "docker" } },
    { command: "mvn spring-boot:run", opts: { kind: "java", javaHome: "D:/jdk" } },
  ];

  it.each(commands)("$command 在内置终端执行，不读取旧的关闭设置", async ({ command, opts }) => {
    const store = useTerminalStore();

    await store.run(project, command, opts);

    expect(mocks.runCommandSession).toHaveBeenCalledExactlyOnceWith(project, command, opts);
    expect(mocks.runInTerminal).not.toHaveBeenCalled();
    expect(mocks.useSettingsStore).not.toHaveBeenCalled();
    expect(store.activeId).toBe(session.id);
    expect(store.open).toBe(true);
    expect(mocks.onListen).toHaveBeenCalledTimes(2);
  });

  it("未传选项时使用默认参数，连续运行只初始化一次事件订阅", async () => {
    const store = useTerminalStore();

    await store.run(project, "echo first");
    await store.run(project, "echo second");

    expect(mocks.runCommandSession).toHaveBeenNthCalledWith(1, project, "echo first", {});
    expect(mocks.runCommandSession).toHaveBeenNthCalledWith(2, project, "echo second", {});
    expect(mocks.listCommandSessions).toHaveBeenCalledTimes(1);
    expect(mocks.onListen).toHaveBeenCalledTimes(2);
  });

  it("内置会话启动失败时传播错误，不回退系统终端或打开空面板", async () => {
    const store = useTerminalStore();
    const error = new Error("启动终端失败");
    mocks.runCommandSession.mockRejectedValueOnce(error);

    await expect(store.run(project, "npm run dev")).rejects.toBe(error);

    expect(mocks.runInTerminal).not.toHaveBeenCalled();
    expect(store.activeId).toBeNull();
    expect(store.open).toBe(false);
  });

  it("仍可手动创建指定 Shell 的内置交互式终端", async () => {
    const store = useTerminalStore();

    await store.create(project, "powershell");

    expect(mocks.createShellSession).toHaveBeenCalledExactlyOnceWith(project, "powershell");
    expect(store.activeId).toBe(43);
    expect(store.open).toBe(true);
  });
});
