//! Demo binary: runs the full TUI against a scripted mock provider (no network).
//! Used by the PTY parity test to drive the real terminal loop + rendering.

use async_trait::async_trait;

use flows_core::models::{GlobalConfig, PhaseConfig, ProjectConfig, ToolsConfig};
use flows_core::state::Store;
use flows_llm::provider::Emit;
use flows_llm::types::{Completion, CompletionRequest, LlmError, Usage};
use flows_llm::Provider;

/// Streams a known line then returns an artifact so the phase completes.
struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    async fn complete(&self, _req: &CompletionRequest) -> Result<Completion, LlmError> {
        Ok(artifact())
    }
    async fn complete_streaming(
        &self,
        _req: &CompletionRequest,
        on_token: Emit<'_>,
        _on_thinking: Emit<'_>,
    ) -> Result<Completion, LlmError> {
        for chunk in ["SELECTABLE-TEXT-0123456789\n", "second demo line\n"] {
            on_token(chunk);
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        Ok(artifact())
    }
}

fn artifact() -> Completion {
    Completion {
        content: "```json\n{\"type\": \"artifact\", \"content\": {\"ok\": true}}\n```".into(),
        reasoning: None,
        tool_calls: vec![],
        usage: Usage::default(),
        stop_reason: None,
    }
}

fn main() {
    let tmp = std::env::temp_dir().join(format!("flows-tui-demo-{}", std::process::id()));
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let store = Store::new(tmp.join(".flows"));

    let mut phase = PhaseConfig::new("demo");
    phase.tools = ToolsConfig {
        files: false,
        ..Default::default()
    };

    let _ = flows_tui::run_tui(
        "demo-run".into(),
        "demo idea".into(),
        repo,
        vec![phase],
        "feature".into(),
        GlobalConfig::default(),
        ProjectConfig {
            llm: Default::default(),
            phases: vec![],
            template: String::new(),
            context: Default::default(),
        },
        Box::new(MockProvider),
        store,
        None,
    );
}
