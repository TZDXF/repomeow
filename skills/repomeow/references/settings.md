# AI 配置、提示词、Git 账号与项目文件

通过 PATH 直接调用 `repomeow`。

## AI 接入配置与诊断

```powershell
# 读取完整配置(含 apiKey 明文,注意保密;文件缺失时自动播种)
repomeow ai config-get

# 全量覆盖配置:先 config-get 导出编辑,再写回(仅在明确要求覆盖时执行)
repomeow ai config-save --file <配置.json>

# 内置厂商目录(添加厂商的候选清单,含预置模型)
repomeow ai builtin-providers

# 列出默认模型所属厂商的可用模型 / 测试默认模型连通性
repomeow ai models
repomeow ai test
```

`config-get` 和导出的配置 JSON 含 apiKey 明文,不要写入日志或提交到仓库;临时配置文件使用后安全删除。

## AI 用量

```powershell
# 汇总:总调用/token/耗时、按任务类型与按日分布
repomeow ai usage-summary

# 明细日志(分页;--task-type 过滤,如 report/wiki/chat/commit)
repomeow ai usage-log [--offset 0] [--limit 50] [--task-type report]

# 清空
repomeow ai usage-clear
```

## AI 提示词

```powershell
# 当前自定义提示词(空串 = 使用内置默认)
repomeow prompt get

# 内置默认模板(只读预览)
repomeow prompt default

# 从文件设置;未指定项保持现状,--clear 将未指定项恢复默认
repomeow prompt set [--commit-file commit.md] [--report-file report.md] [--report-weekly-file weekly.md] [--clear]
```

## Git 平台账号

```powershell
repomeow account list
repomeow account add --provider github|gitee|gitlab --label 备注 --token <令牌> [--base-url https://gitlab.example.com]
repomeow account update --id 1 --label 新备注 [--token <新令牌>] [--base-url ...]
repomeow account remove --id 1
```

`add`/`update` 会先调平台 API 验证 token,验证失败不落库。github/gitee 用固定地址,`--base-url` 仅 gitlab 需要(自建实例)。`--token` 可能进入 shell 历史或进程参数,执行时应注意终端与日志环境,不要将 token 写入技能或仓库。

## 项目文件

```powershell
# 列出一层条目(默认根目录;--dir 指定子目录)
repomeow file list -p <项目目录> [--dir src/components]

# 文件名模糊搜索
repomeow file search -p <项目目录> -q "mod.rs" [--limit 20]

# 全文搜索(glob 过滤与设置页一致)
repomeow file search-text -p <项目目录> -q "TODO" [--include "src/**"] [--exclude "*.lock"] [--case-sensitive] [--whole-word] [--regex]

# 写入项目内文本文件(创建或覆盖;上限 512KB,父目录必须已存在;拒绝绝对路径、..、链接逃逸及已有符号链接目标)
repomeow file save -p <项目目录> --path docs/note.md --content "内容"
repomeow file save -p <项目目录> --path docs/note.md --content-file ./draft.md
```

普通文件搜索和读写优先使用 agent 原生工具;仅在需要 RepoMeow 的 glob 过滤、项目目录约束或统一 JSON 输出时使用本组。`file save` 会覆盖已有文件,仅在用户明确要求时执行。
