# 标签、自定义命令、常用标记、隐藏项与终端 Tab

通过 PATH 直接调用 `repomeow-cli`。`<项目目录>` 指 RepoMeow 登记项目时使用的目录(需未归档)。

## 标签

```powershell
repomeow-cli tag list
repomeow-cli tag create --name 后端 [--color "#ff0000"]
repomeow-cli tag update --id 1 --name 前端 [--color "#00ff00"]
repomeow-cli tag delete --id 1

# 全量覆盖项目的标签绑定;省略 --tag-ids 即清空
repomeow-cli tag set-project -p <项目目录> --tag-ids 1,2
repomeow-cli tag set-project -p <项目目录>
```

颜色格式 `#RGB`/`#RRGGBB`/`#RRGGBBAA`,缺省 `#3b82f6`。删除标签会同时解除各项目绑定。

## 自定义命令

```powershell
repomeow-cli script list -p <项目目录>
repomeow-cli script create -p <项目目录> --name dev --command "pnpm dev" [--description 描述] [--icon rocket]
repomeow-cli script update --id 1 --name dev --command "pnpm dev" [--description 描述] [--icon rocket]
repomeow-cli script delete --id 1
```

`update` 全量提交 name/command,description/icon 缺省会清空(与设置页编辑一致);命令名项目内唯一,重名报 `command_name_conflict`。删除命令会连带移除其常用标记,仅在用户明确要求时执行。

## 常用命令标记(pin)

```powershell
# 列出标记(缺省全部项目;-p 限定单个)
repomeow-cli pin list [-p <项目目录>]

# 标记/取消
repomeow-cli pin set -p <项目目录> --kind <类型> --target-key <标识> --pinned true [--label 名] [--command 内容] [--cwd 目录]
repomeow-cli pin set -p <项目目录> --kind <类型> --target-key <标识> --pinned false
```

kind 取值:`packageScript`(target-key 为 `<package目录>:<脚本名>`)、`composeFile`、`composeService`、`customCommand`(target-key 为命令 id)、`javaBuild`。标记后出现在系统托盘弹窗的快捷入口。

## 隐藏项

```powershell
repomeow-cli hidden list -p <项目目录>
repomeow-cli hidden set -p <项目目录> --kind <类型> --target-key <标识> --hidden true|false
```

kind 取值:`packageFile` / `packageScript` / `composeFile` / `javaBuild`。隐藏的条目不再在项目详情页展示。

## 桌面终端 Tab

```powershell
# 列出 Tab(-p 可限定项目),获取后续操作需要的 id
repomeow-cli terminal list [-p <项目目录>]
repomeow-cli terminal output --id 1 [-p <项目目录>]

# 向交互终端输入文本;--enter 追加回车(仅发送回车可用 --text "" --enter)
repomeow-cli terminal input --id 1 --text "y" [--enter] [-p <项目目录>]

# 只能执行项目已有命令,由桌面端验证目标并生成最终命令
repomeow-cli terminal create npm -p <项目目录> --name dev [--package .]
repomeow-cli terminal create custom -p <项目目录> --id 1

# 启动已停止/退出的 Tab;停止;强制重启
repomeow-cli terminal start --id 1 [-p <项目目录>]
repomeow-cli terminal stop --id 1 [-p <项目目录>]
repomeow-cli terminal restart --id 1 [-p <项目目录>]
```

`output` 可读取当前输出缓冲;`input` 写入运行中 Tab,已结束时返回 `written=false`。输入、停止和重启前确认目标 Tab ID 与项目,必要时用 `-p` 限定,避免操作用户正在使用的其他终端。这些命令会启动或唤醒桌面应用,任务显示在应用内终端中,方便用户手动接管,不会在 CLI 后台隐藏运行。`create` 不接受任意命令文本,也不提供 Docker 操作;重复创建同一 npm/custom 命令会复用并重启原 Tab。`input/start/restart` 仅允许 npm/custom 类型 Tab,不能操作 Shell/Agent/Docker Tab 绕过限制。`start` 不会重复启动已运行 Tab;`restart` 总是按原命令和目录重启。终端会话仅存内存,桌面应用退出后清空。
