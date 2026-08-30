import type { Ref } from "vue";

// 本地最小定义,替代 AI SDK `ai` 包的 FileUIPart。
// RepoMeow 暂未启用附件上传,仅为保持生成代码的类型完整性。
export interface FileUIPart {
  type: "file";
  url?: string;
  mediaType?: string;
  filename?: string;
}

/** 提交状态(本地定义,替代 AI SDK `ai` 包的 ChatStatus) */
export type ChatStatus = "submitted" | "streaming" | "ready" | "error";

export interface PromptInputMessage {
  text: string;
  files: FileUIPart[];
}

export interface AttachmentFile extends FileUIPart {
  id: string;
  file?: File;
}

export interface PromptInputContext {
  textInput: Ref<string>;
  files: Ref<AttachmentFile[]>;
  isLoading: Ref<boolean>;
  fileInputRef: Ref<HTMLInputElement | null>;
  setTextInput: (val: string) => void;
  addFiles: (files: File[] | FileList) => void;
  removeFile: (id: string) => void;
  clearFiles: () => void;
  clearInput: () => void;
  openFileDialog: () => void;
  submitForm: () => void;
}

export const PROMPT_INPUT_KEY = Symbol("PromptInputContext");
