import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { router } from "./router";
import { i18n } from "./i18n";
import { getEditorIcons } from "./lib/open-with";
import { enableDeveloperMode } from "@/lib/developer-mode";
import { useSettingsStore } from "@/stores/settings";
import "@fontsource/nunito/400.css";
import "@fontsource/nunito/600.css";
import "@fontsource/nunito/700.css";
import "@fontsource/nunito/800.css";
import "@fontsource/zen-maru-gothic/400.css";
import "@fontsource/zen-maru-gothic/500.css";
import "@fontsource/zen-maru-gothic/700.css";
import "./style.css";
import "./styles/markdown/index.css";
import "vue-sonner/style.css";

const app = createApp(App);
app.use(createPinia());
app.use(router);
app.use(i18n);
app.mount("#app");

// 应用启动即提前发起编辑器真实图标提取(fire-and-forget,结果走 open-with.ts 的模块级缓存)。
// 否则 OpenWithIcon 挂载时才请求,会排在项目页数据请求之后、并争抢后端 DB 互斥锁,图标迟迟不出。
void getEditorIcons();

// 开发者模式:dev 构建恒启用;release 构建按 settings.json 的 developerMode 开关启用
// (F12 切换 DevTools + Ctrl+Shift+E 元素源码选取)。element-source-dev 经动态 import
// 加载,未开启时仅为异步 chunk,不进首屏 bundle。store init 幂等,与 App.vue 共用实例。
if (import.meta.env.DEV) {
  void enableDeveloperMode();
} else {
  const settingsStore = useSettingsStore();
  void settingsStore.init().then(() => {
    if (settingsStore.developerMode) {
      void enableDeveloperMode();
    }
  });
}

// 仅在打包版本禁用 WebView 默认右键菜单;dev 保留以便右键检查元素调试。
// import.meta.env.DEV 由 Vite 静态替换,生产构建中此分支整体被消除,无运行时代价。
if (!import.meta.env.DEV) {
  window.addEventListener("contextmenu", (e) => e.preventDefault());
}
