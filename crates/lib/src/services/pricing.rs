//! Per-model pricing for cost estimation.
//!
//! Prices are in USD per million tokens. Updated periodically
//! as providers adjust pricing.

/// Pricing per million tokens.
struct ModelPricing {
    input_per_m: f64,
    output_per_m: f64,
    cache_read_per_m: f64,
    cache_write_per_m: f64,
}

/// Calculate USD cost from token usage.
pub fn calculate_cost(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> f64 {
    let pricing = pricing_for_model(model);
    let input_cost = input_tokens as f64 * pricing.input_per_m / 1_000_000.0;
    let output_cost = output_tokens as f64 * pricing.output_per_m / 1_000_000.0;
    let cache_read_cost = cache_read_tokens as f64 * pricing.cache_read_per_m / 1_000_000.0;
    let cache_write_cost = cache_write_tokens as f64 * pricing.cache_write_per_m / 1_000_000.0;
    input_cost + output_cost + cache_read_cost + cache_write_cost
}

fn pricing_for_model(model: &str) -> ModelPricing {
    let lower = model.to_lowercase();

    // Anthropic models.
    if lower.contains("opus") {
        return ModelPricing {
            input_per_m: 15.0,
            output_per_m: 75.0,
            cache_read_per_m: 1.5,
            cache_write_per_m: 18.75,
        };
    }
    if lower.contains("sonnet") {
        return ModelPricing {
            input_per_m: 3.0,
            output_per_m: 15.0,
            cache_read_per_m: 0.3,
            cache_write_per_m: 3.75,
        };
    }
    if lower.contains("haiku") {
        return ModelPricing {
            input_per_m: 0.25,
            output_per_m: 1.25,
            cache_read_per_m: 0.03,
            cache_write_per_m: 0.3,
        };
    }

    // OpenAI models.
    if lower.contains("gpt-5.4") && !lower.contains("mini") && !lower.contains("nano") {
        return ModelPricing {
            input_per_m: 2.50,
            output_per_m: 10.0,
            cache_read_per_m: 1.25,
            cache_write_per_m: 2.50,
        };
    }
    if lower.contains("gpt-5.4-mini") {
        return ModelPricing {
            input_per_m: 0.40,
            output_per_m: 1.60,
            cache_read_per_m: 0.20,
            cache_write_per_m: 0.40,
        };
    }
    if lower.contains("gpt-5.4-nano") {
        return ModelPricing {
            input_per_m: 0.10,
            output_per_m: 0.40,
            cache_read_per_m: 0.05,
            cache_write_per_m: 0.10,
        };
    }
    if lower.contains("gpt-4.1") && !lower.contains("mini") && !lower.contains("nano") {
        return ModelPricing {
            input_per_m: 2.0,
            output_per_m: 8.0,
            cache_read_per_m: 0.50,
            cache_write_per_m: 2.0,
        };
    }
    if lower.contains("gpt-4.1-mini") {
        return ModelPricing {
            input_per_m: 0.40,
            output_per_m: 1.60,
            cache_read_per_m: 0.10,
            cache_write_per_m: 0.40,
        };
    }
    if lower.contains("gpt-4.1-nano") {
        return ModelPricing {
            input_per_m: 0.10,
            output_per_m: 0.40,
            cache_read_per_m: 0.025,
            cache_write_per_m: 0.10,
        };
    }
    if lower.starts_with("o3") || lower.starts_with("o1") {
        return ModelPricing {
            input_per_m: 10.0,
            output_per_m: 40.0,
            cache_read_per_m: 2.50,
            cache_write_per_m: 10.0,
        };
    }
    if lower.contains("gpt-4o") {
        return ModelPricing {
            input_per_m: 2.50,
            output_per_m: 10.0,
            cache_read_per_m: 1.25,
            cache_write_per_m: 2.50,
        };
    }

    // xAI/Grok.
    if lower.contains("grok") {
        return ModelPricing {
            input_per_m: 3.0,
            output_per_m: 15.0,
            cache_read_per_m: 0.0,
            cache_write_per_m: 0.0,
        };
    }

    // Google Gemini.
    if lower.contains("gemini") && lower.contains("pro") {
        return ModelPricing {
            input_per_m: 1.25,
            output_per_m: 5.0,
            cache_read_per_m: 0.0,
            cache_write_per_m: 0.0,
        };
    }
    if lower.contains("gemini") && lower.contains("flash") {
        return ModelPricing {
            input_per_m: 0.15,
            output_per_m: 0.60,
            cache_read_per_m: 0.0,
            cache_write_per_m: 0.0,
        };
    }

    // DeepSeek.
    if lower.contains("deepseek") {
        return ModelPricing {
            input_per_m: 0.27,
            output_per_m: 1.10,
            cache_read_per_m: 0.07,
            cache_write_per_m: 0.27,
        };
    }

    // Mistral.
    if lower.contains("mistral") && lower.contains("large") {
        return ModelPricing {
            input_per_m: 2.0,
            output_per_m: 6.0,
            cache_read_per_m: 0.0,
            cache_write_per_m: 0.0,
        };
    }

    // Local/unknown — free.
    ModelPricing {
        input_per_m: 0.0,
        output_per_m: 0.0,
        cache_read_per_m: 0.0,
        cache_write_per_m: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sonnet_pricing() {
        let cost = calculate_cost("claude-sonnet-4-20250514", 1_000_000, 100_000, 0, 0);
        // 1M input * $3/M + 100K output * $15/M = $3 + $1.5 = $4.5
        assert!((cost - 4.5).abs() < 0.01);
    }

    #[test]
    fn test_unknown_model_free() {
        let cost = calculate_cost("local-llama", 1_000_000, 1_000_000, 0, 0);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_gpt4_1_mini() {
        let cost = calculate_cost("gpt-4.1-mini", 1_000_000, 0, 0, 0);
        assert!((cost - 0.40).abs() < 0.01);
    }

    #[test]
    fn test_opus_pricing() {
        let cost = calculate_cost("claude-opus-4", 1_000_000, 0, 0, 0);
        assert!((cost - 15.0).abs() < 0.01);
    }

    #[test]
    fn test_haiku_pricing() {
        let cost = calculate_cost("claude-haiku-4", 0, 1_000_000, 0, 0);
        assert!((cost - 1.25).abs() < 0.01);
    }

    #[test]
    fn test_cache_pricing() {
        // Sonnet: cache read = $0.3/M, cache write = $3.75/M
        let cost = calculate_cost("claude-sonnet-4", 0, 0, 1_000_000, 1_000_000);
        assert!((cost - (0.3 + 3.75)).abs() < 0.01);
    }

    #[test]
    fn test_deepseek_pricing() {
        let cost = calculate_cost("deepseek-v3", 1_000_000, 0, 0, 0);
        assert!((cost - 0.27).abs() < 0.01);
    }

    #[test]
    fn test_grok_pricing() {
        let cost = calculate_cost("grok-3", 1_000_000, 0, 0, 0);
        assert!((cost - 3.0).abs() < 0.01);
    }

    #[test]
    fn test_zero_tokens() {
        let cost = calculate_cost("claude-sonnet-4", 0, 0, 0, 0);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_gemini_pro_pricing() {
        let cost = calculate_cost("gemini-pro", 1_000_000, 0, 0, 0);
        assert!((cost - 1.25).abs() < 0.01);
    }

    #[test]
    fn test_gemini_flash_pricing() {
        let cost = calculate_cost("gemini-flash", 1_000_000, 0, 0, 0);
        assert!((cost - 0.15).abs() < 0.01);
    }

    #[test]
    fn test_gpt4o_pricing() {
        let cost = calculate_cost("gpt-4o", 1_000_000, 0, 0, 0);
        assert!((cost - 2.50).abs() < 0.01);
    }

    #[test]
    fn test_gpt54_pricing() {
        let cost = calculate_cost("gpt-5.4", 1_000_000, 0, 0, 0);
        assert!((cost - 2.50).abs() < 0.01);
    }

    #[test]
    fn test_gpt54_mini_pricing() {
        let cost = calculate_cost("gpt-5.4-mini", 1_000_000, 0, 0, 0);
        assert!((cost - 0.40).abs() < 0.01);
    }

    #[test]
    fn test_gpt54_nano_pricing() {
        let cost = calculate_cost("gpt-5.4-nano", 1_000_000, 0, 0, 0);
        assert!((cost - 0.10).abs() < 0.01);
    }

    #[test]
    fn test_gpt41_pricing() {
        let cost = calculate_cost("gpt-4.1", 1_000_000, 0, 0, 0);
        assert!((cost - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_gpt41_nano_pricing() {
        let cost = calculate_cost("gpt-4.1-nano", 1_000_000, 0, 0, 0);
        assert!((cost - 0.10).abs() < 0.01);
    }

    #[test]
    fn test_o3_pricing() {
        let cost = calculate_cost("o3", 1_000_000, 0, 0, 0);
        assert!((cost - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_o1_pricing() {
        let cost = calculate_cost("o1", 1_000_000, 0, 0, 0);
        assert!((cost - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_mistral_large_pricing() {
        let cost = calculate_cost("mistral-large", 1_000_000, 0, 0, 0);
        assert!((cost - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_combined_input_output_cost() {
        // Opus: 1M input * $15/M + 500K output * $75/M = $15 + $37.5 = $52.5
        let cost = calculate_cost("claude-opus-4", 1_000_000, 500_000, 0, 0);
        assert!((cost - 52.5).abs() < 0.01);
    }

    #[test]
    fn test_grok_cache_pricing_is_zero() {
        // Grok has zero cache rates.
        let cost = calculate_cost("grok-3", 0, 0, 1_000_000, 1_000_000);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_gemini_cache_pricing_is_zero() {
        // Gemini has zero cache rates.
        let cost = calculate_cost("gemini-pro", 0, 0, 1_000_000, 1_000_000);
        assert_eq!(cost, 0.0);
    }
}
