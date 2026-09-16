---
name: repomeow
description: 用 RepoMeow CLI 操作 RepoMeow 管理的本地项目:Git 状态与提交、Wiki 查询、代码语义分析、项目数据只读查询、生成日报/周报。当任务涉及 RepoMeow 登记项目的提交、文档、实体级代码分析或工作报告时使用。
---

# RepoMeow CLI

可执行文件:`{{REPOMEOW_CLI}}`(下文以 `repomeow` 代称)。首参数传入子命令即进入 CLI 模式,不打开窗口。

约定:成功结果以 JSON 写 stdout;失败以含 code/message/detail 字段的 JSON 写 stderr 且退出码非 0。`repomeow <分组> --help` 可查看各命令参数。

## 命令分组

| 分组 | 用途 | 参考 |
| --- | --- | --- |
| `git` | 仓库状态摘要、创建提交(全部或指定文件) | [references/git.md](references/git.md) |
| `wiki` / `project` | Wiki 目录/大纲/页面正文、项目文件按行读取、报告历史、自定义命令(均只读) | [references/project.md](references/project.md) |
| `sem` | 语义搜索代码实体、实体上下文、调用/引用关系、未提交变更汇总 | [references/sem.md](references/sem.md) |
| `report` | 为已登记项目生成日报/周报(调用 AI 并写入报告历史,消耗 AI 额度) | [references/report.md](references/report.md) |

## 通用注意

- `<项目目录>` 指 RepoMeow 登记项目时使用的目录;未登记或已归档返回 `project_not_found`。
- `git`/`sem` 的 `-d` 参数可传仓库内任意路径,操作始终以仓库根目录为准。
- 数据目录默认 `~/.repomeow`,可用环境变量 `REPOMEOW_DATA_DIR` 覆盖。
