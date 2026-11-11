# 语义分析

通过 PATH 直接调用 `repomeow-cli`。以下命令均为只读,`-d` 可传仓库内任意路径。

## 命令

```powershell
# 按名称语义搜索代码实体(函数/类/接口/结构体等)
repomeow-cli sem find -d <仓库目录> -q <关键词>

# 查看实体上下文:源码摘要 + 按调用/引用扩展的相关实体
repomeow-cli sem context -d <仓库目录> -e <实体名或entityId> [--file-path src/a.ts] [--budget 2000] [--hops 1]

# 查询实体的直接调用方(callers)与引用点(refs)
repomeow-cli sem relations -d <仓库目录> -e <实体名或entityId> [--file-path src/a.ts]

# 汇总当前未提交的实体级结构化变更
repomeow-cli sem diff -d <仓库目录>
```

## 使用建议

- 实体参数:含 `::` 的串视为 entityId(形如 `src/a.ts::function::run`)精确匹配,否则按实体名匹配;重名时用 `--file-path` 消歧。
- 典型流程:`sem find` 定位实体 → `sem context` 理解实现 → `sem relations` 评估影响面。
- `sem` 命令直接调用 RepoMeow 内置的 sem sidecar 分析 Git 工作区,不要求项目已登记或先在桌面应用中打开;二进制缺失或目录不是 Git 仓库时会返回错误。
