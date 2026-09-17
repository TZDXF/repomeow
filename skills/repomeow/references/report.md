# 报告生成、历史与调度

通过 PATH 直接调用 `repomeow`。

## 生成报告

```powershell
# 生成今日日报(单个项目)
repomeow report generate -p <项目目录> --period-type daily

# 生成最近 7 天周报(多个项目,逗号分隔)
repomeow report generate -p <项目目录1>,<项目目录2> --period-type weekly

# 指定时间范围/作者/语言
repomeow report generate -p <项目目录> --period-type daily --date-from 2026-09-01 --date-to 2026-09-15 --author-mode me --language zh-CN
```

参数:

- `--period-type`:`daily`(日报,缺省范围=今天)/ `weekly`(周报,缺省范围=最近 7 天)。
- `--author-mode`:`all` 全部提交(默认)/ `me` 仅当前 git 用户。
- `--language`:`zh-CN`(默认)/ `en-US`。

## 报告历史

```powershell
# 列表(-p 可省略;更多用 project reports)
repomeow project reports [-p <项目目录>] [--limit 10]

# 单条详情(含 Markdown 正文与提交记录)
repomeow report history-get --id <报告id>

# 删除(级联删除关联提交记录)
repomeow report history-delete --id <报告id>
```

## 报告调度

```powershell
# 列出全部定时报告调度
repomeow report schedules

# 全量覆盖:先 schedules 导出编辑,再写回(JSON 数组)
repomeow report schedules-save --file <调度.json>

# 系统级调度(当前仅 git_update 后台检查)
repomeow report system-schedules
repomeow report system-schedule-save --enabled true --interval-minutes 10
```

`--interval-minutes` 范围 1-1440。调度变更写入数据库后,**运行中的桌面应用需重启才被调度器感知**(CLI 进程退出时无法通知其调度循环)。

## 注意

- 项目必须已在 RepoMeow 登记且未归档,否则返回 `project_not_found`。
- 生成是同步等待的(通常十几秒到一分钟),会消耗用户在 RepoMeow 配置的 AI 额度;请确保 RepoMeow 已配置可用的 AI 接入。
- 范围内无提交时不生成(`generated=false`)。成功后报告写入报告历史。