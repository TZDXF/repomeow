import { createRouter, createWebHashHistory } from "vue-router";
import ProjectsHome from "@/views/ProjectsHome.vue";
import ProjectDetail from "@/views/ProjectDetail.vue";
import Settings from "@/views/Settings.vue";
import ReportHistory from "@/views/ReportHistory.vue";
import TrayPopup from "@/views/TrayPopup.vue";

// 文件预览页与提交图(CommitDetailPanel)引用完整的 vscode-icons 图标集(约 3.5MB),懒加载避免拖累首屏
const ProjectFiles = () => import("@/views/ProjectFiles.vue");
const GitGraph = () => import("@/views/GitGraph.vue");

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "home", component: ProjectsHome },
    { path: "/projects/:id", name: "project", component: ProjectDetail },
    { path: "/projects/:id/files", name: "project-files", component: ProjectFiles },
    { path: "/projects/:id/graph", name: "project-graph", component: GitGraph },
    { path: "/settings", name: "settings", component: Settings },
    { path: "/report-history", name: "history", component: ReportHistory },
    // 托盘迷你弹窗窗口加载 index.html#/tray 进入该路由
    { path: "/tray", name: "tray", component: TrayPopup },
  ],
});
