//! Turning an `agent` row's provider and model into the expression `agent.ts` uses.
//!
//! Only the providers that can be generated *correctly* are generated (D11). Emitting
//! eve's `anthropic()` or `openai()` helpers would be worse than refusing: both read an
//! API key from the environment, which is exactly the metered path D8 exists to
//! prevent, and a Claude node wired without the `createAiSdkMcpServer` bridge would
//! reach the model with no tools at all.

use crate::error::{Error, Result};
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
}

/// The ailocal gateway. Free, on loopback, and allowed by every machine profile.
const AILOCAL_BASE_URL: &str = "http://127.0.0.1:8081/v1";
/// z.ai's GLM Coding Plan, openai-compatible and flat-rate.
const ZAI_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";

pub(super) fn model_expression(agent: &Agent) -> Result<ModelExpression> {
    match agent.provider {
        Provider::Local => Ok(openai_compatible(
            "ailocal",
            AILOCAL_BASE_URL,
            "AI_TEAM_AILOCAL_KEY",
            &agent.model,
        )),
        Provider::ZAi => Ok(openai_compatible(
            "zai",
            ZAI_BASE_URL,
            "AI_TEAM_ZAI_KEY",
            &agent.model,
        )),
        // Both are real subscription paths, and both need a slice's worth of work that
        // has not happened yet. Refusing names the slice rather than shipping something
        // that looks wired and is not.
        Provider::Claude => Err(Error::invalid(format!(
            "agent {:?} is on the Claude subscription, which reaches eve through the \
             createAiSdkMcpServer bridge - that is M1-S6. Until then, point it at `local` \
             or `zai`.",
            agent.role
        ))),
        Provider::OpenAi => Err(Error::invalid(format!(
            "agent {:?} is on the ChatGPT subscription, whose credential path under \
             `eve start` is unproven - that is M1-S5. Until then, point it at `local` or \
             `zai`.",
            agent.role
        ))),
    }
}

/// Both remaining providers speak the OpenAI wire format, so they differ only in base
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
    }
}

/// What to tell eve when nobody has established the model's real context window.
///
/// eve refuses to compile compaction for a model it cannot size, and it can only size AI
/// Gateway model IDs - which D8 guarantees we never use. So a number is always required.
/// 32k is deliberately conservative: compacting sooner than necessary costs a summary
/// call, while overflowing the window fails the turn. M1-S5 resolves the real value from
/// the provider; until then this is the floor a modern local model comfortably clears.
pub(super) const FALLBACK_CONTEXT_WINDOW: i64 = 32_768;

/// The context window to emit for an agent.
pub(super) fn context_window(agent: &Agent) -> i64 {
    agent.context_window.unwrap_or(FALLBACK_CONTEXT_WINDOW)
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
        let expr = model_expression(&agent_on(Provider::Local, "auto")).unwrap();
        assert!(expr.expression.contains("127.0.0.1:8081"));
        assert!(expr.expression.contains("\"auto\""));
        assert_eq!(expr.env, ["AI_TEAM_AILOCAL_KEY"]);
    }

    #[test]
    fn zai_points_at_the_coding_plan_endpoint() {
        let expr = model_expression(&agent_on(Provider::ZAi, "glm-4.6")).unwrap();
        assert!(expr.expression.contains("api.z.ai/api/coding/paas/v4"));
        assert!(expr.expression.contains("\"glm-4.6\""));
    }

    #[test]
    fn no_generated_expression_ever_names_a_metered_key() {
        // The regression this guards: someone "fixes" an unsupported provider by
        // reaching for eve's anthropic()/openai() helpers, which read these.
        for provider in [Provider::Local, Provider::ZAi] {
            let expr = model_expression(&agent_on(provider, "m")).unwrap();
            let rendered = format!("{} {}", expr.imports.join(" "), expr.expression);
            assert!(!rendered.contains("OPENAI_API_KEY"), "{rendered}");
            assert!(!rendered.contains("ANTHROPIC_API_KEY"), "{rendered}");
            assert!(!rendered.contains("AI_GATEWAY_API_KEY"), "{rendered}");
        }
    }

    #[test]
    fn the_unbuilt_providers_refuse_and_name_their_slice() {
        let claude = model_expression(&agent_on(Provider::Claude, "sonnet")).unwrap_err();
        assert!(claude.to_string().contains("M1-S6"), "{claude}");
        let openai = model_expression(&agent_on(Provider::OpenAi, "gpt-5.6")).unwrap_err();
        assert!(openai.to_string().contains("M1-S5"), "{openai}");
    }

    #[test]
    fn a_model_name_with_a_quote_in_it_cannot_break_out_of_the_literal() {
        // Model names come from the database, and the database is edited by a human.
        let expr = model_expression(&agent_on(Provider::Local, "we\"ird")).unwrap();
        assert!(
            expr.expression.contains(r#""we\"ird""#),
            "{}",
            expr.expression
        );
    }
}
