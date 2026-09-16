//! 内置 skill:技能文本位于仓库根目录 `skills/repomeow/`,随二进制打包,可在「设置 → CLI」一键导入资源库,
//! 再由项目 AI 资源部署到各 agent 的 skills 目录。
//!
//! 渐进式披露:SKILL.md 为入口与分组路由,细分用法在 references/ 下。
//! 文件中的 `{{REPOMEOW_CLI}}` 占位符在导入时替换为当前可执行文件绝对路径。

/// 内置 skill 定义。
pub(crate) struct BuiltinSkill {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    /// (技能目录内相对路径, 文件内容);必含 SKILL.md 且位于首位。
    pub(crate) files: &'static [(&'static str, &'static str)],
}

pub(crate) const EXECUTABLE_PLACEHOLDER: &str = "{{REPOMEOW_CLI}}";

pub(crate) const BUILTIN_SKILLS: &[BuiltinSkill] = &[BuiltinSkill {
    name: "repomeow",
    description: "RepoMeow CLI:Git / Wiki / 语义分析 / 项目数据 / 日报周报",
    files: &[
        ("SKILL.md", include_str!("../../../skills/repomeow/SKILL.md")),
        (
            "references/git.md",
            include_str!("../../../skills/repomeow/references/git.md"),
        ),
        (
            "references/project.md",
            include_str!("../../../skills/repomeow/references/project.md"),
        ),
        (
            "references/sem.md",
            include_str!("../../../skills/repomeow/references/sem.md"),
        ),
        (
            "references/report.md",
            include_str!("../../../skills/repomeow/references/report.md"),
        ),
    ],
}];
