//! Port of `flows/pricing.py` — model pricing table and cost calculation.
//! Prices are USD per 1M tokens. Lookup: exact → longest-prefix → None ($0).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_1m: f64,
    pub output_per_1m: f64,
}

impl ModelPricing {
    const fn new(input_per_1m: f64, output_per_1m: f64) -> Self {
        Self {
            input_per_1m,
            output_per_1m,
        }
    }
}

/// (normalised model name, pricing). Kept as a slice so lookup can do the same
/// exact-then-longest-prefix match as the Python dict.
const PRICES: &[(&str, ModelPricing)] = &[
    // Anthropic
    ("claude-haiku-4-5", ModelPricing::new(0.80, 4.00)),
    ("claude-sonnet-4-6", ModelPricing::new(3.00, 15.00)),
    ("claude-opus-4-8", ModelPricing::new(15.00, 75.00)),
    ("claude-fable-5", ModelPricing::new(3.00, 15.00)),
    ("claude-3-5-haiku", ModelPricing::new(1.00, 5.00)),
    ("claude-3-5-sonnet", ModelPricing::new(3.00, 15.00)),
    ("claude-3-opus", ModelPricing::new(15.00, 75.00)),
    ("claude-3-sonnet", ModelPricing::new(3.00, 15.00)),
    ("claude-3-haiku", ModelPricing::new(0.25, 1.25)),
    // OpenAI
    ("gpt-4o-mini", ModelPricing::new(0.15, 0.60)),
    ("gpt-4o", ModelPricing::new(2.50, 10.00)),
    ("gpt-4-turbo", ModelPricing::new(10.00, 30.00)),
    ("gpt-4", ModelPricing::new(30.00, 60.00)),
    ("gpt-3.5-turbo", ModelPricing::new(0.50, 1.50)),
    ("o1-mini", ModelPricing::new(3.00, 12.00)),
    ("o1", ModelPricing::new(15.00, 60.00)),
    ("o3-mini", ModelPricing::new(1.10, 4.40)),
    ("o3", ModelPricing::new(10.00, 40.00)),
    // Google
    ("gemini-2.5-pro", ModelPricing::new(1.25, 10.00)),
    ("gemini-2.0-flash", ModelPricing::new(0.10, 0.40)),
    ("gemini-1.5-pro", ModelPricing::new(1.25, 5.00)),
    ("gemini-1.5-flash", ModelPricing::new(0.075, 0.30)),
    // Mistral
    ("mistral-large", ModelPricing::new(3.00, 9.00)),
    ("mistral-medium", ModelPricing::new(2.70, 8.10)),
    ("mistral-small", ModelPricing::new(1.00, 3.00)),
    ("codestral", ModelPricing::new(1.00, 3.00)),
    // Local / free
    ("ollama", ModelPricing::new(0.00, 0.00)),
    ("ollama_chat", ModelPricing::new(0.00, 0.00)),
    ("lm-studio", ModelPricing::new(0.00, 0.00)),
    ("local", ModelPricing::new(0.00, 0.00)),
];

const PROVIDER_PREFIXES: &[&str] = &[
    "anthropic/",
    "openai/",
    "google/",
    "gemini/",
    "mistral/",
    "vertex_ai/",
    "azure/",
    "bedrock/",
    "cohere/",
];

fn normalise(model: &str) -> String {
    let m = model.trim().to_lowercase();
    for prefix in PROVIDER_PREFIXES {
        if let Some(rest) = m.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    m
}

/// Return pricing for `model`, or None if unknown.
pub fn lookup_pricing(model: &str) -> Option<ModelPricing> {
    let key = normalise(model);

    // Local/Ollama fast-path.
    for p in ["ollama/", "ollama_chat/", "ollama", "lm-studio", "local"] {
        if key.starts_with(p) {
            return Some(ModelPricing::new(0.0, 0.0));
        }
    }

    // Exact match.
    if let Some((_, pricing)) = PRICES.iter().find(|(name, _)| *name == key) {
        return Some(*pricing);
    }

    // Longest-prefix match (versioned names like claude-sonnet-4-6-20250219).
    let mut best: Option<ModelPricing> = None;
    let mut best_len = 0;
    for (pattern, pricing) in PRICES {
        if key.starts_with(pattern) && pattern.len() > best_len {
            best = Some(*pricing);
            best_len = pattern.len();
        }
    }
    best
}

/// USD cost. Returns 0.0 for unknown models (not None).
pub fn calculate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    match lookup_pricing(model) {
        None => 0.0,
        Some(p) => {
            input_tokens as f64 * p.input_per_1m / 1_000_000.0
                + output_tokens as f64 * p.output_per_1m / 1_000_000.0
        }
    }
}

/// Output throughput. Returns 0.0 if latency is zero.
pub fn tokens_per_second(output_tokens: u64, latency_ms: i64) -> f64 {
    if latency_ms <= 0 {
        return 0.0;
    }
    output_tokens as f64 / (latency_ms as f64 / 1000.0)
}

pub fn format_cost(usd: f64) -> String {
    if usd == 0.0 {
        "$0.000000".to_string()
    } else if usd < 0.01 {
        format!("${usd:.6}")
    } else {
        format!("${usd:.4}")
    }
}
