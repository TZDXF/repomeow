//! system prompt 中的技能清单格式化:对齐 `packages/agent/src/harness/system-prompt.ts`。

use crate::agent::harness::types::Skill;

/// 生成 agentskills.io 规范兼容的 `<available_skills>` 系统提示块。
/// `disableModelInvocation` 的技能被排除;无可见技能时返回空串。
pub fn format_skills_for_system_prompt(skills: &[Skill]) -> String {
    let visible_skills: Vec<&Skill> = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation.unwrap_or(false))
        .collect();
    if visible_skills.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = vec![
        "The following skills provide specialized instructions for specific tasks.".to_string(),
        "Read the full skill file when the task matches its description.".to_string(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in visible_skills {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path)
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

/// XML 五个保留字符转义(对齐 TS `escapeXml`)。
pub fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, description: &str, disabled: bool) -> Skill {
        Skill {
            name: name.to_string(),
            description: description.to_string(),
            content: String::new(),
            file_path: "/skills/commit/SKILL.md".to_string(),
            disable_model_invocation: if disabled { Some(true) } else { None },
        }
    }

    #[test]
    fn empty_when_no_visible_skills() {
        assert_eq!(format_skills_for_system_prompt(&[]), "");
        assert_eq!(format_skills_for_system_prompt(&[skill("x", "d", true)]), "");
    }

    #[test]
    fn escapes_and_lists_skills() {
        let output = format_skills_for_system_prompt(&[skill("com<mit>", "uses \"quotes\"", false)]);
        assert!(output.contains("<name>com&lt;mit&gt;</name>"));
        assert!(output.contains("<description>uses &quot;quotes&quot;</description>"));
        assert!(output.starts_with("The following skills provide specialized instructions"));
        assert!(output.ends_with("</available_skills>"));
    }
}
