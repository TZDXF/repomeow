---
name: repomeow
description: 用 RepoMeow CLI 管理 RepoMeow 的本地开发项目:项目登记/归档、Wiki 查询与配置、代码语义分析、标签/自定义命令/常用标记/隐藏项、日报周报与调度、AI 配置/提示词/用量、Git 平台账号、项目文件搜索与写入。当任务涉及 RepoMeow 登记项目的管理或自动化操作时使用。
---

# RepoMeow CLI

通过 PATH 直接调用 `repomeow`,无需指定可执行文件路径。首参数传入子命令即进入 CLI 模式,不打开窗口。

约定:成功结果以 JSON 写 stdout;失败以含 code/message/detail 字段的 JSON 写 stderr 且退出码非 0。`repomeow <分组> --help` 可查看各命令参数。

## 环境准备

使用前需将 RepoMeow 程序目录加入 PATH。Windows 可在「设置 → CLI」点击「添加到用户 PATH」,然后重启终端和 AI 工具。若提示找不到 `repomeow`,请先完成上述配置,不要将本机绝对路径写入技能。

## 命令分组

| 分组 | 用途 | 参考 |
| --- | --- | --- |
| `wiki` / `project` | Wiki 查询与配置;项目登记管理(增删改查/归档/收藏/开关)、文件按行读取 | [references/project.md](references/project.md) |
| `sem` | 语义搜索代码实体、实体上下文、调用/引用关系、未提交变更汇总 | [references/sem.md](references/sem.md) |
| `report` | 生成日报/周报、报告历史、报告调度与系统调度 | [references/report.md](references/report.md) |
| `tag` / `script` / `pin` / `hidden` | 标签、自定义命令、常用命令标记、条目隐藏 | [references/manage.md](references/manage.md) |
| `ai` / `prompt` / `account` / `file` | AI 配置/模型/连接测试/用量、提示词、Git 平台账号、项目文件搜索与写入 | [references/settings.md](references/settings.md) |

## 通用注意

- `<项目目录>` 指 RepoMeow 登记项目时使用的目录;`project`/`tag`/`script`/`pin`/`hidden`/`report` 的项目级操作要求项目已登记且未归档,否则返回 `project_not_found`(项目自身的 add/get/archive/unarchive/delete 除外)。
- `sem` 的 `-d` 参数可传仓库内任意路径,操作始终以仓库根目录为准。
- 数据目录默认 `~/.repomeow`,可用环境变量 `REPOMEOW_DATA_DIR` 覆盖。
- CLI 与运行中的桌面应用共享同一 SQLite 与配置文件:数据立即可见,但调度类变更(报告调度、系统调度)需应用重启后才被调度器感知。