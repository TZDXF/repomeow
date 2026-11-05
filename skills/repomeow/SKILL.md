---
name: repomeow
description: 用 RepoMeow CLI 管理 RepoMeow 的本地开发项目:项目登记/归档、Wiki 生成/更新/查询与配置、代码语义分析、标签/自定义命令/常用标记/隐藏项、日报周报与调度、AI 配置/提示词/用量、Git 平台账号、项目文件搜索与写入、桌面终端 Tab 启停。当任务涉及 RepoMeow 登记项目的管理或自动化操作时使用。
---

# RepoMeow CLI

通过 PATH 直接调用 `repomeow`,无需指定可执行文件路径。首参数传入子命令即进入 CLI 模式;普通命令不打开窗口,`terminal` 命令会启动或唤醒桌面应用。

约定:命令成功结果以 JSON 写 stdout(`--help` / `--version` 为纯文本);失败以含 code/message/detail 字段的 JSON 写 stderr 且退出码非 0。`repomeow <分组> --help` 可查看各命令参数。

## 环境准备

使用前需将包含 `repomeow` 可执行文件的目录加入 PATH。Windows 可在「设置 → CLI」点击「添加到用户 PATH」,然后重启终端和 AI 工具;macOS / Linux 按实际安装位置手动配置 PATH。若提示找不到 `repomeow`,请先完成上述配置,不要将本机绝对路径写入技能。

## 命令分组

| 分组 | 用途 | 参考 |
| --- | --- | --- |
| `wiki` / `project` | Wiki 生成/更新/查询与配置;项目登记管理(增删改查/归档/收藏/开关)、文件按行读取 | [references/project.md](references/project.md) |
| `sem` | 语义搜索代码实体、实体上下文、调用/引用关系、未提交变更汇总 | [references/sem.md](references/sem.md) |
| `report` | 生成日报/周报、报告历史与报告调度;另含 Git 后台检查的系统调度 | [references/report.md](references/report.md) |
| `tag` / `script` / `pin` / `hidden` | 标签、自定义命令、常用命令标记、条目隐藏 | [references/manage.md](references/manage.md) |
| `ai` / `prompt` / `account` / `file` | AI 配置/模型/连接测试/用量、提示词、Git 平台账号、项目文件搜索与写入 | [references/settings.md](references/settings.md) |
| `terminal` | 桌面终端 Tab 列表、输出、输入、创建、启动、停止与重启 | [references/manage.md](references/manage.md) |

## 通用注意

- 需要登记状态的 `project`/`tag`/`script`/`pin`/`hidden`/`report` 项目级操作要求项目已登记且未归档,否则返回 `project_not_found`(`project get/archive/unarchive/delete` 可操作已归档项目)。`project read-file`、Wiki 与 `file` 直接按路径操作,不要求登记;`terminal create` 与终端 `-p` 过滤要求已登记且未归档。
- `sem` 的 `-d` 参数可传仓库内任意路径,操作始终以仓库根目录为准。
- 数据目录默认 `~/.repomeow`,可用环境变量 `REPOMEOW_DATA_DIR` 覆盖。
- CLI 与运行中的桌面应用共享同一 SQLite 与配置文件:数据立即可见,但调度类变更(报告调度、系统调度)需应用重启后才被调度器感知。
- 删除、清空、全量覆盖配置/调度、整本 Wiki 重生成、文件覆盖及终端输入/启停仅在用户明确要求的范围内执行;报告生成前确认日期与作者口径,避免错误消耗 AI 额度。
- AI 配置和账号 token 属于敏感信息,不要写入日志、临时技能或仓库;注意 `--token` 可能进入 shell 历史。普通文件读写优先使用 agent 原生工具,只有需要 RepoMeow 的文件过滤或项目目录约束时才使用 `file`。
