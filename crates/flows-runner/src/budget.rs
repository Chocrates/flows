//! Context-window budgeting for the agent tool loop (Tier 2). As tool results
//! accumulate, the request can approach the model's context window. Before each
//! send, estimate the request size and, when it exceeds a configured fraction of
//! the window, **truncate the oldest tool-result bodies** to a small stub — always
//! preserving the system prompt, the task (first message), and the most recent
//! turns. This keeps the loop alive on small/local windows instead of overflowing.
//!
//! Token counts are a cheap heuristic (~4 chars/token), not a real BPE tokenizer:
//! budgeting only needs to know *when we're near the edge*, and a heuristic avoids
//! a heavyweight tokenizer dependency. It errs slightly high (safe: trims sooner).

use flows_llm::types::{Message, Role};

/// Rough token estimate for a string (~4 chars/token, rounded up). Shared with
/// retrieval packing so both budgets use the same yardstick.
pub fn est_str(s: &str) -> usize {
    s.len() / 4 + 1
}

/// Estimated tokens for one message: content + a rough allowance for each tool
/// call's name + serialized arguments.
pub fn estimate_message_tokens(m: &Message) -> usize {
    let mut n = m.content.as_deref().map(est_str).unwrap_or(0);
    for tc in &m.tool_calls {
        n += est_str(&tc.name) + est_str(&tc.arguments) + 4;
    }
    n + 4 // per-message role/framing overhead
}

/// Estimated tokens for the whole request (system prompt + all messages).
pub fn estimate_request_tokens(system: &str, messages: &[Message]) -> usize {
    est_str(system) + messages.iter().map(estimate_message_tokens).sum::<usize>()
}

/// Trim old tool-result bodies until the estimated request fits `budget_fraction`
/// of `window`, preserving the system prompt, the first message (the task), and
/// the last `recent_keep` messages. Truncates oldest eligible `Tool` messages
/// first, replacing the body with a `[truncated N tokens]` stub. Returns the
/// number of estimated tokens reclaimed.
pub fn trim_to_budget(
    messages: &mut [Message],
    system: &str,
    window: u64,
    budget_fraction: f64,
    recent_keep: usize,
) -> usize {
    let limit = (window as f64 * budget_fraction).max(0.0) as usize;
    let mut total = estimate_request_tokens(system, messages);
    if total <= limit {
        return 0;
    }
    let n = messages.len();
    // Eligible to trim: not the first message (task), not within the last
    // `recent_keep`, and a Tool result with a real body.
    let last_protected = n.saturating_sub(recent_keep);
    let mut reclaimed = 0;
    for (i, m) in messages.iter_mut().enumerate().skip(1) {
        if total <= limit || i >= last_protected {
            break; // under budget, or reached the protected recent tail
        }
        if m.role != Role::Tool {
            continue;
        }
        let Some(body) = m.content.as_ref() else {
            continue;
        };
        let before = est_str(body);
        if before <= 8 {
            continue; // already tiny — not worth stubbing
        }
        let stub = format!("[truncated {before} tokens]");
        let after = est_str(&stub);
        m.content = Some(stub);
        let saved = before.saturating_sub(after);
        total = total.saturating_sub(saved);
        reclaimed += saved;
    }
    reclaimed
}

#[cfg(test)]
mod tests {
    use super::*;
    use flows_llm::types::Message;

    fn big_tool(id: &str, chars: usize) -> Message {
        Message::tool_result(id, "x".repeat(chars))
    }

    #[test]
    fn no_trim_when_under_budget() {
        let mut msgs = vec![Message::user("task"), big_tool("a", 40)];
        let saved = trim_to_budget(&mut msgs, "sys", 100_000, 0.6, 2);
        assert_eq!(saved, 0);
        assert!(msgs[1].content.as_ref().unwrap().len() == 40);
    }

    #[test]
    fn trims_oldest_tool_results_preserving_head_and_recent() {
        // Window 1000, fraction 0.5 → limit ~500 tokens (~2000 chars).
        let mut msgs = vec![
            Message::user("the task"),            // 0: protected (head)
            big_tool("t1", 4000),                 // 1: oldest tool → trimmed
            big_tool("t2", 4000),                 // 2: trimmed if still over
            Message::assistant_text("reasoning"), // 3
            big_tool("t3", 4000),                 // 4: recent tail → protected
        ];
        let saved = trim_to_budget(&mut msgs, "sys", 1000, 0.5, 2);
        assert!(saved > 0, "should have trimmed");
        // Head + recent tail intact.
        assert_eq!(msgs[0].content.as_deref(), Some("the task"));
        assert_eq!(msgs[4].content.as_ref().unwrap().len(), 4000);
        // Oldest tool result was stubbed.
        assert!(msgs[1].content.as_ref().unwrap().starts_with("[truncated"));
    }

    #[test]
    fn recent_tail_never_trimmed_even_if_over() {
        // Everything but head is within recent_keep → nothing trimmable.
        let mut msgs = vec![Message::user("task"), big_tool("t", 8000)];
        let saved = trim_to_budget(&mut msgs, "sys", 100, 0.5, 4);
        assert_eq!(saved, 0);
        assert_eq!(msgs[1].content.as_ref().unwrap().len(), 8000);
    }
}
