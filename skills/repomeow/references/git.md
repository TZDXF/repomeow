# Git

通过 PATH 直接调用 `repomeow`。

## 查看状态

```powershell
repomeow git status -d <仓库目录>
```

返回当前分支、与上游的领先/落后提交数、暂存/未暂存修改/未跟踪/冲突文件数、最后抓取与提交时间;非 git 目录返回 `isRepo=false`。

## 创建提交

```powershell
# 提交全部变更(含未跟踪文件)
repomeow git commit -d <仓库目录> -m "feat: 提交信息"

# 仅提交指定文件(仓库相对路径,逗号分隔或重复 --files)
repomeow git commit -d <仓库目录> -m "fix: 提交信息" --files src/a.ts,src/b.ts
```

`--files` 只接受仓库相对路径(`/` 分隔),拒绝绝对路径与含 `..` 的路径。成功返回 `commitHash`、`shortHash`、`branch`、`committedFiles`。

## 注意

- 提交前先 `git status` 确认变更范围,避免误提交。
- 提交范围始终以仓库根目录为准,`-d` 可传仓库内任意路径。
