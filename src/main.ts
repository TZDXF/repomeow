import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { router } from "./router";
import { i18n } from "./i18n";
import { getEditorIcons } from "./lib/open-with";
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

// 仅在开发模式加载 element-source-dev:Ctrl+Shift+E 切换元素选取模式,点击页面元素可查看其
// 源文件位置与组件栈,方便开发时定位组件代码。import.meta.env.DEV 由 Vite 静态替换,
// 生产构建中此分支连同 element-source / html2canvas 依赖被整体 tree-shake,不进产物。
if (import.meta.env.DEV) {
  const { default: elementDev } = await import("element-source-dev");
  elementDev();
}

// 仅在打包版本禁用 WebView 默认右键菜单;dev 保留以便右键检查元素调试。
// import.meta.env.DEV 由 Vite 静态替换,生产构建中此分支整体被消除,无运行时代价。
if (!import.meta.env.DEV) {
  window.addEventListener("contextmenu", (e) => e.preventDefault());
}
