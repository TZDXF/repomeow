# 标签、自定义命令、常用标记与隐藏项

通过 PATH 直接调用 `repomeow`。`<项目目录>` 指 RepoMeow 登记项目时使用的目录(需未归档)。

## 标签

```powershell
repomeow tag list
repomeow tag create --name 后端 [--color "#ff0000"]
repomeow tag update --id 1 --name 前端 [--color "#00ff00"]
repomeow tag delete --id 1

# 全量覆盖项目的标签绑定(传空列表即清空)
repomeow tag set-project -p <项目目录> --tag-ids 1,2
```

颜色格式 `#RGB`/`#RRGGBB`/`#RRGGBBAA`,缺省 `#3b82f6`。删除标签会同时解除各项目绑定。

## 自定义命令

```powershell
repomeow script list -p <项目目录>
repomeow script create -p <项目目录> --name dev --command "pnpm dev" [--description 描述] [--icon rocket]
repomeow script update --id 1 --name dev --command "pnpm dev" [--description 描述] [--icon rocket]
repomeow script delete --id 1
```

`update` 全量提交 name/command(与设置页编辑一致);命令名项目内唯一,重名报 `command_name_conflict`。删除命令会连带移除其常用标记。

## 常用命令标记(pin)

```powershell
# 列出标记(缺省全部项目;-p 限定单个)
repomeow pin list [-p <项目目录>]

# 标记/取消
repomeow pin set -p <项目目录> --kind <类型> --target-key <标识> --pinned true [--label 名] [--command 内容] [--cwd 目录]
repomeow pin set -p <项目目录> --kind <类型> --target-key <标识> --pinned false
```

kind 取值:`packageScript`(target-key 为 `<package目录>:<脚本名>`)、`composeFile`、`composeService`、`customCommand`(target-key 为命令 id)、`javaBuild`。标记后出现在系统托盘弹窗的快捷入口。

## 隐藏项

```powershell
repomeow hidden list -p <项目目录>
repomeow hidden set -p <项目目录> --kind <类型> --target-key <标识> --hidden true|false
```

kind 取值:`packageFile` / `packageScript` / `composeFile` / `javaBuild`。隐藏的条目不再在项目详情页展示。