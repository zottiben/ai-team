//! Turning an `agent` row's provider and model into the expression `agent.ts` uses.
//!
//! Only the providers that can be generated *correctly* are generated (D11). In
//! particular, eve exports both `chatgpt()` (the local subscription token broker) and
//! `openai()` (a metered API key) from the same module. The spelling here is a security
//! boundary, not a convenience.

use crate::model::{Agent, Provider};

/// What an agent's model needs in the generated `agent.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelExpression {
    /// Import lines to place at the top of the file.
    pub imports: Vec<String>,
    /// The expression assigned to `model:`.
    pub expression: String,
    /// Environment variables the process must carry for this to resolve.
    pub env: Vec<&'static str>,
    /// Claude Code runs its own loop, so its model also needs the authored tools handed
    /// to it as an in-process MCP server. Other AI SDK providers use eve's normal path.
    pub bridge_tools: bool,
}

/// z.ai's GLM Coding Plan, openai-compatible and flat-rate.
const ZAI_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";

pub(super) fn model_expression(agent: &Agent, ailocal_base_url: &str) -> ModelExpression {
    match agent.provider {
        Provider::Local => openai_compatible(
            "ailocal",
            ailocal_base_url,
            "AI_TEAM_AILOCAL_KEY",
            &agent.model,
        ),
        Provider::ZAi => openai_compatible("zai", ZAI_BASE_URL, "AI_TEAM_ZAI_KEY", &agent.model),
        Provider::OpenAi => ModelExpression {
            imports: vec!["import { chatgpt } from \"eve/models/openai\";".to_string()],
            expression: format!("chatgpt({:?})", agent.model),
            env: Vec::new(),
            bridge_tools: false,
        },
        Provider::Claude => ModelExpression {
            imports: vec!["import { claudeCode, createAiSdkMcpServer } from \
                 \"ai-sdk-provider-claude-code\";"
                .to_string()],
            expression: format!("claudeCode({:?}, claudeSettings)", agent.model),
            env: Vec::new(),
            bridge_tools: true,
        },
    }
}

/// The compatible providers speak the OpenAI wire format, so they differ only in base
/// URL and key.
fn openai_compatible(
    name: &str,
    base_url: &str,
    key_env: &'static str,
    model: &str,
) -> ModelExpression {
    ModelExpression {
        imports: vec![
            "import { createOpenAICompatible } from \"@ai-sdk/openai-compatible\";".to_string(),
        ],
        expression: format!(
            "createOpenAICompatible({{\n    name: {name:?},\n    baseURL: {base_url:?},\n    \
             apiKey: process.env.{key_env} ?? \"\",\n  }})({model:?})"
        ),
        env: vec![key_env],
        bridge_tools: false,
    }
}

/// The context window to emit for an agent.
///
/// eve refuses to compile compaction for a model it cannot size, and it can only size AI
/// Gateway model IDs - which D8 guarantees we never use. Explicit team metadata wins;
/// otherwise the subscription registry supplies the selected provider's conservative
/// default (200k for Claude, 32k for the other current integrations).
pub(super) fn context_window(agent: &Agent) -> i64 {
    agent
        .context_window
        .unwrap_or_else(|| crate::machine::provider_default(agent.provider).1)
}

/// eve's own spelling of reasoning effort. `Reasoning` mirrors it exactly, so this is a
/// straight mapping rather than a judgement.
pub(super) fn reasoning_literal(agent: &Agent) -> &'static str {
    agent.reasoning.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Reasoning;
    use crate::roles::preset;

    fn agent_on(provider: Provider, model: &str) -> Agent {
        let new = preset("backend").unwrap().to_new_agent(0);
        Agent {
            id: 1,
            team_id: 1,
            ord: 0,
            role: new.role,
            name: new.name,
            purpose: new.purpose,
            provider,
            model: model.to_string(),
            reasoning: Reasoning::Medium,
            zone: new.zone,
            prompt_preset: new.prompt_preset,
            prompt_md: None,
            context_window: None,
            read_only: false,
            enabled: true,
            rev: 1,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn local_points_at_the_ailocal_gateway_on_loopback() {
        let expr = model_expression(
            &agent_on(Provider::Local, "auto"),
            "http://127.0.0.1:9191/v1",
        );
        assert!(expr.expression.contains("127.0.0.1:9191"));
        assert!(expr.expression.contains("\"auto\""));
        assert_eq!(expr.env, ["AI_TEAM_AILOCAL_KEY"]);
    }

    #[test]
    fn zai_points_at_the_coding_plan_endpoint() {
        let expr = model_expression(
            &agent_on(Provider::ZAi, "glm-4.6"),
            "http://127.0.0.1:8081/v1",
        );
        assert!(expr.expression.contains("api.z.ai/api/coding/paas/v4"));
        assert!(expr.expression.contains("\"glm-4.6\""));
    }

    #[test]
    fn no_generated_expression_ever_names_a_metered_key() {
        // The regression this guards: someone "fixes" an unsupported provider by
        // reaching for eve's anthropic()/openai() helpers, which read these.
        for provider in [
            Provider::Claude,
            Provider::OpenAi,
            Provider::Local,
            Provider::ZAi,
        ] {
            let expr = model_expression(&agent_on(provider, "m"), "http://127.0.0.1:8081/v1");
            let rendered = format!("{} {}", expr.imports.join(" "), expr.expression);
            assert!(!rendered.contains("OPENAI_API_KEY"), "{rendered}");
            assert!(!rendered.contains("ANTHROPIC_API_KEY"), "{rendered}");
            assert!(!rendered.contains("AI_GATEWAY_API_KEY"), "{rendered}");
        }
    }

    #[test]
    fn chatgpt_is_the_subscription_helper_not_the_metered_openai_helper() {
        let expr = model_expression(
            &agent_on(Provider::OpenAi, "gpt-5.6-luna-fast"),
            "http://127.0.0.1:8081/v1",
        );
        assert!(expr.imports[0].contains("chatgpt"));
        assert!(expr.expression.starts_with("chatgpt("));
        assert!(expr.env.is_empty());
        assert!(!expr.bridge_tools);
    }

    #[test]
    fn claude_is_the_subscription_provider_with_the_explicit_tool_bridge() {
        let expr = model_expression(
            &agent_on(Provider::Claude, "sonnet"),
            "http://127.0.0.1:8081/v1",
        );
        assert!(expr.imports[0].contains("ai-sdk-provider-claude-code"));
        assert_eq!(expr.expression, "claudeCode(\"sonnet\", claudeSettings)");
        assert!(expr.env.is_empty());
        assert!(expr.bridge_tools);
    }

    #[test]
    fn a_model_name_with_a_quote_in_it_cannot_break_out_of_the_literal() {
        // Model names come from the database, and the database is edited by a human.
        let expr = model_expression(
            &agent_on(Provider::Local, "we\"ird"),
            "http://127.0.0.1:8081/v1",
        );
        assert!(
            expr.expression.contains(r#""we\"ird""#),
            "{}",
            expr.expression
        );
    }
}
