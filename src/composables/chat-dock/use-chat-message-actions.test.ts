import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { computed, ref } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { sendChatMessage, type ChatEvent } from "@/lib/chat";
import { useChatStore } from "@/stores/chat";
import type { Project } from "@/types";
import { useChatMessageActions } from "./use-chat-message-actions";

// 只模拟后端传输,项目选择与会话落点使用真实 composable/store。
vi.mock("@/lib/chat", () => ({
  sendChatMessage: vi.fn(
    async (
      _project: { path: string; name: string },
      params: { message: string; onEvent: (event: ChatEvent) => void },
    ) => {
      params.onEvent({ kind: "textDelta", delta: `回复:${params.message}` });
      params.onEvent({ kind: "turnEnd", contextTokens: null });
      return null;
    },
  ),
  newChatSession: vi.fn(async () => {}),
  respondToolPermission: vi.fn(async () => true),
  truncateChatLastTurn: vi.fn(async () => {}),
}));

function harness() {
  const chat = useChatStore();
  const project = ref({ path: "D:/repos/one", name: "one" } as Project);
  const session = computed(() => chat.ensureSession(project.value.path));
  const aiReady = ref(true);
  const actions = useChatMessageActions({
    project: () => project.value,
    session,
    aiReady: computed(() => aiReady.value),
  });
  const send = vi.spyOn(chat, "send").mockResolvedValue(true);
  return { chat, project, session, aiReady, actions, send };
}

describe("useChatMessageActions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
  });

  it("切换项目后发送使用当前项目,不被旧项目忙状态静默拒绝", async () => {
    const { chat, project, actions, send } = harness();
    chat.ensureSession(project.value.path).busy = true;
    project.value = { ...project.value, path: "D:/repos/two", name: "two" };

    await expect(actions.onSubmit({ text: "  新项目的问题  ", files: [] })).resolves.toBe(true);
    expect(send).toHaveBeenCalledWith(project.value.path, project.value, "新项目的问题");
  });

  it("仅切换不同项目 tab 的路由,每个项目独立保存提问和回答,切回后不串会话", async () => {
    const projects = new Map<number, Project>([
      [1, { id: 1, path: "D:/repos/one", name: "one" } as Project],
      [2, { id: 2, path: "D:/repos/two", name: "two" } as Project],
      [3, { id: 3, path: "D:/repos/three", name: "three" } as Project],
    ]);
    // 与 TitleTabs.openProject 一致:同一路由记录只切换项目 id,不重建发送逻辑。
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: "/projects/:id", component: { render: () => null } }],
    });
    await router.push("/projects/1");
    const chat = useChatStore();
    const project = computed(() => projects.get(Number(router.currentRoute.value.params.id))!);
    const session = computed(() => chat.ensureSession(project.value.path));
    const actions = useChatMessageActions({
      project: () => project.value,
      session,
      aiReady: computed(() => true),
    });

    const visits = [1, 2, 3, 1];
    for (const [index, id] of visits.entries()) {
      await router.push(`/projects/${id}`);
      await expect(actions.onSubmit({ text: `问题${index + 1}`, files: [] })).resolves.toBe(true);
      expect(session.value).toBe(chat.sessions[projects.get(id)!.path]);
    }

    expect(chat.sessions[projects.get(1)!.path]!.messages.map((m) => m.content)).toEqual([
      "问题1",
      "回复:问题1",
      "问题4",
      "回复:问题4",
    ]);
    expect(chat.sessions[projects.get(2)!.path]!.messages.map((m) => m.content)).toEqual([
      "问题2",
      "回复:问题2",
    ]);
    expect(chat.sessions[projects.get(3)!.path]!.messages.map((m) => m.content)).toEqual([
      "问题3",
      "回复:问题3",
    ]);
    expect(vi.mocked(sendChatMessage).mock.calls.map(([target]) => target.path)).toEqual(
      visits.map((id) => projects.get(id)!.path),
    );
  });

  it("工作树路径变化后停止/新会话/编辑重发均使用当前路径", () => {
    const { chat, project, actions } = harness();
    const abort = vi.spyOn(chat, "abort").mockImplementation(() => {});
    const newSession = vi.spyOn(chat, "newSession").mockResolvedValue();
    const edit = vi.spyOn(chat, "editLastUserMessage").mockResolvedValue(true);
    project.value = { ...project.value, path: "D:/worktrees/feature" };

    actions.abort();
    actions.startNewSession();
    actions.startEdit({ key: "u1", content: "原问题" });
    actions.editText.value = "修改后的问题";
    actions.confirmEdit();
    expect(abort).toHaveBeenCalledWith(project.value.path);
    expect(newSession).toHaveBeenCalledWith(project.value.path);
    expect(edit).toHaveBeenCalledWith(project.value.path, project.value, "修改后的问题");
  });

  it("配置不可用/当前会话忙时返回拒绝并展示原因", () => {
    const { actions, session, aiReady, send } = harness();
    aiReady.value = false;
    expect(actions.onSubmit({ text: "问题", files: [] })).toBe(false);
    expect(session.value.error).toBeTruthy();
    const configurationError = session.value.error;

    aiReady.value = true;
    session.value.busy = true;
    expect(actions.sendText("问题")).toBe(false);
    expect(session.value.error).toBeTruthy();
    expect(session.value.error).not.toBe(configurationError);
    expect(send).not.toHaveBeenCalled();
  });

  it("空白消息不发送;store 的拒绝结果传回输入组件", async () => {
    const { actions, send } = harness();
    expect(actions.sendText("  \n ")).toBe(false);
    expect(send).not.toHaveBeenCalled();
    send.mockResolvedValue(false);
    await expect(actions.onSubmit({ text: "问题", files: [] })).resolves.toBe(false);
  });
});
