# 报告生成

通过 PATH 直接调用 `repomeow`。

## 命令

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

## 注意

- 项目必须已在 RepoMeow 登记且未归档,否则返回 `project_not_found`。
- 生成是同步等待的(通常十几秒到一分钟),会消耗用户在 RepoMeow 配置的 AI 额度;请确保 RepoMeow 已配置可用的 AI 接入。
- 范围内无提交时不生成(`generated=false`)。成功后报告写入 RepoMeow 报告历史,可用 `repomeow project reports` 查询。
