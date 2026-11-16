import { defineAsyncComponent, defineComponent, h } from "vue";
import { useI18n } from "vue-i18n";
import { loadUpdateMarkdown } from "./update-markdown-loader";

export default defineComponent({
  name: "UpdateNotes",
  props: {
    content: { type: String, required: true },
  },
  setup(props) {
    const { t } = useI18n(),
      UpdateNotesLoading = defineComponent({
        setup: () => () =>
          h(
            "p",
            { class: "text-sm text-muted-foreground", role: "status" },
            t("update.notesLoading"),
          ),
      }),
      UpdateNotesFallback = defineComponent({
        setup: () => () =>
          h(
            "p",
            { class: "whitespace-pre-line break-words text-sm text-muted-foreground" },
            props.content,
          ),
      }),
      // 按需拆包，避免把代码高亮等重依赖放入首屏；加载失败时保留原始说明。
      SafeMarkdown = defineAsyncComponent({
        loader: loadUpdateMarkdown,
        loadingComponent: UpdateNotesLoading,
        errorComponent: UpdateNotesFallback,
        delay: 0,
      });

    return () => h(SafeMarkdown, { mode: "static", content: props.content });
  },
});
