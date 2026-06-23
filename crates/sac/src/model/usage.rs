use super::*;

pub(super) fn parse_chat_usage(value: Option<&Value>) -> Usage {
    Usage {
        prompt_tokens: value
            .and_then(|usage| usage.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        completion_tokens: value
            .and_then(|usage| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        total_tokens: value
            .and_then(|usage| usage.get("total_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        reasoning_tokens: value
            .and_then(|usage| usage.get("completion_tokens_details"))
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        cached_tokens: value
            .and_then(|usage| usage.get("prompt_tokens_details"))
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
    }
}

pub(super) fn parse_responses_usage(value: Option<&Value>) -> Usage {
    Usage {
        prompt_tokens: value
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        completion_tokens: value
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        total_tokens: value
            .and_then(|usage| usage.get("total_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        reasoning_tokens: value
            .and_then(|usage| usage.get("output_tokens_details"))
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        cached_tokens: value
            .and_then(|usage| usage.get("input_tokens_details"))
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_usage_extracts_cached_tokens() {
        let usage_json = json!({
            "prompt_tokens": 500,
            "completion_tokens": 100,
            "total_tokens": 600,
            "prompt_tokens_details": {
                "cached_tokens": 400
            },
            "completion_tokens_details": {
                "reasoning_tokens": 30
            }
        });
        let usage = parse_chat_usage(Some(&usage_json));
        assert_eq!(usage.prompt_tokens, Some(500));
        assert_eq!(usage.completion_tokens, Some(100));
        assert_eq!(usage.total_tokens, Some(600));
        assert_eq!(usage.reasoning_tokens, Some(30));
        assert_eq!(usage.cached_tokens, Some(400));
        // goal_token_delta: (500 - 400) + 100 = 200
        assert_eq!(usage.goal_token_delta(), 200);
    }

    #[test]
    fn chat_usage_no_cached_tokens() {
        let usage_json = json!({
            "prompt_tokens": 500,
            "completion_tokens": 100,
            "total_tokens": 600
        });
        let usage = parse_chat_usage(Some(&usage_json));
        assert_eq!(usage.cached_tokens, None);
        // goal_token_delta without cached: 500 + 100 = 600
        assert_eq!(usage.goal_token_delta(), 600);
    }

    #[test]
    fn responses_usage_extracts_cached_tokens() {
        let usage_json = json!({
            "input_tokens": 1000,
            "output_tokens": 200,
            "total_tokens": 1200,
            "input_tokens_details": {
                "cached_tokens": 800
            },
            "output_tokens_details": {
                "reasoning_tokens": 50
            }
        });
        let usage = parse_responses_usage(Some(&usage_json));
        assert_eq!(usage.prompt_tokens, Some(1000));
        assert_eq!(usage.completion_tokens, Some(200));
        assert_eq!(usage.total_tokens, Some(1200));
        assert_eq!(usage.reasoning_tokens, Some(50));
        assert_eq!(usage.cached_tokens, Some(800));
        // goal_token_delta: (1000 - 800) + 200 = 400
        assert_eq!(usage.goal_token_delta(), 400);
    }

    #[test]
    fn responses_usage_no_cached_tokens() {
        let usage_json = json!({
            "input_tokens": 1000,
            "output_tokens": 200,
            "total_tokens": 1200
        });
        let usage = parse_responses_usage(Some(&usage_json));
        assert_eq!(usage.cached_tokens, None);
        // goal_token_delta without cached: 1000 + 200 = 1200
        assert_eq!(usage.goal_token_delta(), 1200);
    }
}
