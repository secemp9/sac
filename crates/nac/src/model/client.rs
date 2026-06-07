use super::*;
use std::time::Instant;

#[derive(Clone)]
pub struct ModelClient {
    client: Client,
    base_url: String,
    api_key: String,
    pub model: String,
    backend: BackendKind,
    reasoning_effort: Option<ReasoningEffort>,
    reasoning_summary: Option<ReasoningSummary>,
    reasoning_context: Option<ReasoningContext>,
}

impl ModelClient {
    pub fn from_env() -> Result<Self> {
        Self::from_env_with_overrides(ClientOverrides::default())
    }

    pub fn from_env_with_overrides(overrides: ClientOverrides) -> Result<Self> {
        let requested_backend = overrides.backend.unwrap_or(BackendKind::Auto);
        let base_url = overrides.base_url.unwrap_or_else(|| {
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| {
                default_base_url_for_backend_hint(requested_backend).to_string()
            })
        });
        let backend = match requested_backend {
            BackendKind::Auto => detect_backend(&base_url)?,
            explicit => explicit,
        };
        let api_key = api_key_for_backend(
            backend,
            overrides.api_key_env.as_deref(),
            overrides.api_key.as_deref(),
        )?;
        let model = overrides.model.unwrap_or_else(|| {
            std::env::var("OPENAI_MODEL").unwrap_or_else(|_| default_model_for_backend(backend))
        });
        let reasoning_effort = match backend {
            BackendKind::DeepSeekChat => None,
            _ => overrides
                .reasoning_effort
                .or_else(|| default_reasoning_effort(backend)),
        };
        let reasoning_summary = match backend {
            BackendKind::DeepSeekChat | BackendKind::FireworksChat => None,
            _ => overrides.reasoning_summary,
        };
        let reasoning_context = match backend {
            BackendKind::DeepSeekChat | BackendKind::FireworksChat => None,
            _ => overrides.reasoning_context,
        };

        tracing::debug!(
            requested_backend = ?requested_backend,
            resolved_backend = ?backend,
            backend_source = if matches!(requested_backend, BackendKind::Auto) {
                "auto_detect"
            } else {
                "explicit"
            },
            model = %model,
            reasoning_effort = ?reasoning_effort,
            reasoning_summary = ?reasoning_summary,
            reasoning_context = ?reasoning_context,
            "resolved model client configuration"
        );

        Ok(Self {
            client: Client::new(),
            base_url,
            api_key,
            model,
            backend,
            reasoning_effort,
            reasoning_summary,
            reasoning_context,
        })
    }

    pub async fn send_turn(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Result<ModelTurnResponse> {
        let started = Instant::now();
        let message_count = messages.len();
        let tool_count = tools.len();
        tracing::info!(
            backend = ?self.backend,
            model = %self.model,
            reasoning_effort = ?self.reasoning_effort,
            message_count,
            tool_count,
            "starting model turn"
        );

        let response = match self.backend {
            BackendKind::Auto => unreachable!("backend auto should be resolved at client creation"),
            BackendKind::DeepSeekChat => self.send_deepseek_chat(messages, tools).await,
            BackendKind::FireworksChat => self.send_fireworks_chat(messages, tools).await,
            BackendKind::OpenAiResponses => self.send_openai_responses(messages, tools).await,
            BackendKind::ChatGptCodexResponses => {
                chatgpt_codex::send_responses(
                    &self.client,
                    &self.base_url,
                    &self.model,
                    self.reasoning_effort.as_ref(),
                    self.reasoning_summary.as_ref(),
                    self.reasoning_context.as_ref(),
                    messages,
                    tools,
                )
                .await
            }
        }?;

        tracing::info!(
            backend = ?self.backend,
            model = %self.model,
            finish_reason = ?response.finish_reason,
            has_text = response.assistant.content.is_some(),
            tool_call_count = response
                .assistant
                .tool_calls
                .as_ref()
                .map(|calls| calls.len())
                .unwrap_or(0),
            latency_ms = started.elapsed().as_millis() as u64,
            "model turn completed"
        );

        Ok(response)
    }

    pub async fn complete_text(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<TextCompletion> {
        let messages = vec![
            Message::System {
                content: system_prompt.to_string(),
            },
            Message::User {
                content: user_prompt.to_string(),
            },
        ];

        let response = self.send_turn(messages, Vec::new()).await?;
        let content = response
            .assistant
            .content
            .ok_or_else(|| anyhow!("Text completion returned no text content"))?;

        Ok(TextCompletion {
            content,
            usage: response.usage,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn backend(&self) -> BackendKind {
        self.backend
    }

    pub fn reasoning_effort(&self) -> Option<&ReasoningEffort> {
        self.reasoning_effort.as_ref()
    }

    async fn send_fireworks_chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Result<ModelTurnResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut request = json!({
            "model": self.model,
            "messages": messages
                .iter()
                .map(fireworks_message_to_value)
                .collect::<Vec<_>>(),
            "tools": tools,
            "temperature": 0.0
        });

        if let Some(effort) = &self.reasoning_effort {
            match effort {
                ReasoningEffort::Low | ReasoningEffort::Medium | ReasoningEffort::High => {
                    request["reasoning_effort"] = Value::String(effort.as_str().to_string());
                }
                unsupported => {
                    return Err(anyhow!(
                        "reasoning effort '{}' is not supported by fireworks-chat; use low, medium, or high",
                        unsupported.as_str()
                    ));
                }
            }
        }

        tracing::debug!(
            backend = ?self.backend,
            endpoint = "chat_completions",
            request_bytes = json_value_len_bytes(&request)?,
            "built fireworks chat request"
        );
        let value = self.post_json_with_retry(&url, &request).await?;
        parse_chat_completions_response(&value, &url)
    }

    async fn send_deepseek_chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Result<ModelTurnResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let request = deepseek_chat_request(&self.model, &messages, &tools);

        tracing::debug!(
            backend = ?self.backend,
            endpoint = "chat_completions",
            request_bytes = json_value_len_bytes(&request)?,
            "built deepseek chat request"
        );
        let value = self.post_json_with_retry(&url, &request).await?;
        parse_chat_completions_response(&value, &url)
    }

    async fn send_openai_responses(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Result<ModelTurnResponse> {
        let url = format!("{}/responses", self.base_url);
        let mut request = json!({
            "model": self.model,
            "input": responses_input_items(&messages),
        });

        if !tools.is_empty() {
            request["tools"] = Value::Array(
                tools
                    .iter()
                    .map(openai_responses_tool_to_value)
                    .collect::<Vec<_>>(),
            );
        }

        if let Some(effort) = &self.reasoning_effort {
            let mut reasoning = json!({
                "effort": effort.as_str(),
            });
            if let Some(summary) = &self.reasoning_summary {
                reasoning["summary"] = json!(summary.as_str());
            }
            if let Some(context) = &self.reasoning_context {
                reasoning["context"] = json!(context.as_str());
            }
            request["reasoning"] = reasoning;
            request["include"] = json!(["reasoning.encrypted_content"]);
        }

        tracing::debug!(
            backend = ?self.backend,
            endpoint = "responses",
            request_bytes = json_value_len_bytes(&request)?,
            "built openai responses request"
        );
        let value = self.post_json_with_retry(&url, &request).await?;
        parse_openai_responses_response(&value, &url)
    }

    async fn post_json_with_retry(&self, url: &str, body: &Value) -> Result<Value> {
        let mut last_error = anyhow!("No attempts made");
        let request_bytes = json_value_len_bytes(body)?;

        for attempt in 0..3 {
            if attempt > 0 {
                let delay_secs = 1u64 << (attempt - 1);
                tracing::warn!(
                    backend = ?self.backend,
                    endpoint = endpoint_name(url),
                    attempt = attempt + 1,
                    backoff_secs = delay_secs,
                    "retrying model HTTP request after backoff"
                );
                sleep(Duration::from_secs(delay_secs)).await;
            }

            let attempt_started = Instant::now();
            tracing::debug!(
                backend = ?self.backend,
                endpoint = endpoint_name(url),
                attempt = attempt + 1,
                request_bytes,
                "starting model HTTP attempt"
            );

            let response = self
                .client
                .post(url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(body)
                .send()
                .await
                .map_err(|e| anyhow!("HTTP request failed for {}: {}", url, e))?;

            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| anyhow!("Failed to read response body: {}", e))?;
            let response_bytes = body.len();

            if status.is_success() {
                tracing::info!(
                    backend = ?self.backend,
                    endpoint = endpoint_name(url),
                    attempt = attempt + 1,
                    status = status.as_u16(),
                    request_bytes,
                    response_bytes,
                    latency_ms = attempt_started.elapsed().as_millis() as u64,
                    "model HTTP attempt succeeded"
                );
                return serde_json::from_str::<Value>(&body).map_err(|e| {
                    tracing::error!(
                        backend = ?self.backend,
                        endpoint = endpoint_name(url),
                        attempt = attempt + 1,
                        status = status.as_u16(),
                        response_bytes,
                        parse_error = %e,
                        "model HTTP success body failed JSON parse"
                    );
                    anyhow!(
                        "Failed to parse response from {}: {}\nBody: {}",
                        url,
                        e,
                        &body[..body.len().min(500)]
                    )
                });
            }

            if status.as_u16() == 429 || status.is_server_error() {
                tracing::warn!(
                    backend = ?self.backend,
                    endpoint = endpoint_name(url),
                    attempt = attempt + 1,
                    status = status.as_u16(),
                    request_bytes,
                    response_bytes,
                    latency_ms = attempt_started.elapsed().as_millis() as u64,
                    retryable = true,
                    "model HTTP attempt failed with retryable status"
                );
                last_error = anyhow!(
                    "HTTP {} from {}: {}",
                    status.as_u16(),
                    url,
                    &body[..body.len().min(500)]
                );
                continue;
            }

            tracing::error!(
                backend = ?self.backend,
                endpoint = endpoint_name(url),
                attempt = attempt + 1,
                status = status.as_u16(),
                request_bytes,
                response_bytes,
                latency_ms = attempt_started.elapsed().as_millis() as u64,
                retryable = false,
                "model HTTP attempt failed with non-retryable status"
            );
            return Err(anyhow!(
                "HTTP {} from {}: {}",
                status.as_u16(),
                url,
                &body[..body.len().min(500)]
            ));
        }

        Err(last_error)
    }
}

fn json_value_len_bytes(value: &Value) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}

fn endpoint_name(url: &str) -> &'static str {
    if url.contains("/responses") {
        "responses"
    } else if url.contains("/chat/completions") {
        "chat_completions"
    } else {
        "unknown"
    }
}

#[cfg(test)]
impl ModelClient {
    pub fn new_for_test() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "test_dummy_key".to_string(),
            model: "gpt-5.5".to_string(),
            backend: BackendKind::OpenAiResponses,
            reasoning_effort: Some(ReasoningEffort::Xhigh),
            reasoning_summary: None,
            reasoning_context: None,
        }
    }
}
