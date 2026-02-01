use {crate::tool_registry::ToolRegistry, moltis_skills::types::SkillMetadata};

/// Build the system prompt for an agent run, including available tools.
///
/// When `native_tools` is true, tool schemas are sent via the API's native
/// tool-calling mechanism (e.g. OpenAI function calling, Anthropic tool_use).
/// When false, tools are described in the prompt itself and the LLM is
/// instructed to emit tool calls as JSON blocks that the runner can parse.
pub fn build_system_prompt(
    tools: &ToolRegistry,
    native_tools: bool,
    project_context: Option<&str>,
) -> String {
    build_system_prompt_with_session(tools, native_tools, project_context, None, &[])
}

/// Build the system prompt, optionally including session context stats and skills.
pub fn build_system_prompt_with_session(
    tools: &ToolRegistry,
    native_tools: bool,
    project_context: Option<&str>,
    session_context: Option<&str>,
    skills: &[SkillMetadata],
) -> String {
    let tool_schemas = tools.list_schemas();

    let mut prompt = String::from(
        "You are a helpful assistant with access to tools for executing shell commands.\n\n",
    );

    // Inject project context (CLAUDE.md, AGENTS.md, etc.) early so the LLM
    // sees project-specific instructions before tool schemas.
    if let Some(ctx) = project_context {
        prompt.push_str(ctx);
        prompt.push('\n');
    }

    // Inject session context stats so the LLM can answer questions about
    // the current session size and token usage.
    if let Some(ctx) = session_context {
        prompt.push_str("## Current Session\n\n");
        prompt.push_str(ctx);
        prompt.push_str("\n\n");
    }

    // Inject available skills so the LLM knows what skills can be activated.
    if !skills.is_empty() {
        prompt.push_str(&moltis_skills::prompt_gen::generate_skills_prompt(skills));
    }

    if !tool_schemas.is_empty() {
        prompt.push_str("## Available Tools\n\n");
        for schema in &tool_schemas {
            let name = schema["name"].as_str().unwrap_or("unknown");
            let desc = schema["description"].as_str().unwrap_or("");
            let params = &schema["parameters"];
            prompt.push_str(&format!(
                "### {name}\n{desc}\n\nParameters:\n```json\n{}\n```\n\n",
                serde_json::to_string_pretty(params).unwrap_or_default()
            ));
        }
    }

    if !native_tools && !tool_schemas.is_empty() {
        prompt.push_str(concat!(
            "## How to call tools\n\n",
            "To call a tool, output ONLY a JSON block with this exact format (no other text before it):\n\n",
            "```tool_call\n",
            "{\"tool\": \"<tool_name>\", \"arguments\": {<arguments>}}\n",
            "```\n\n",
            "You MUST output the tool call block as the ENTIRE response — do not add any text before or after it.\n",
            "After the tool executes, you will receive the result and can then respond to the user.\n\n",
        ));
    }

    prompt.push_str(concat!(
        "## Guidelines\n\n",
        "- Use the exec tool to run shell commands when the user asks you to perform tasks ",
        "that require system interaction (file operations, running programs, checking status, etc.).\n",
        "- Always explain what you're doing before executing commands.\n",
        "- If a command fails, analyze the error and suggest fixes.\n",
        "- For multi-step tasks, execute commands one at a time and check results before proceeding.\n",
        "- Be careful with destructive operations — confirm with the user first.\n",
    ));

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_prompt_does_not_include_tool_call_format() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt(&tools, true, None);
        assert!(!prompt.contains("```tool_call"));
    }

    #[test]
    fn test_fallback_prompt_includes_tool_call_format() {
        let mut tools = ToolRegistry::new();
        struct Dummy;
        #[async_trait::async_trait]
        impl crate::tool_registry::AgentTool for Dummy {
            fn name(&self) -> &str {
                "test"
            }

            fn description(&self) -> &str {
                "A test tool"
            }

            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {}})
            }

            async fn execute(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }
        tools.register(Box::new(Dummy));

        let prompt = build_system_prompt(&tools, false, None);
        assert!(prompt.contains("```tool_call"));
        assert!(prompt.contains("### test"));
    }

    #[test]
    fn test_skills_injected_into_prompt() {
        let tools = ToolRegistry::new();
        let skills = vec![SkillMetadata {
            name: "commit".into(),
            description: "Create git commits".into(),
            license: None,
            compatibility: None,
            allowed_tools: vec![],
            homepage: None,
            dockerfile: None,
            requires: Default::default(),
            path: std::path::PathBuf::from("/skills/commit"),
            source: None,
        }];
        let prompt = build_system_prompt_with_session(&tools, true, None, None, &skills);
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("commit"));
    }

    #[test]
    fn test_no_skills_block_when_empty() {
        let tools = ToolRegistry::new();
        let prompt = build_system_prompt_with_session(&tools, true, None, None, &[]);
        assert!(!prompt.contains("<available_skills>"));
    }
}
