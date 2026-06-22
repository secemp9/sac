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
    event_sink: Option<EventSink>,
    thread_name: Option<String>,
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
            event_sink: None,
            thread_name: None,
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

    pub fn set_event_sink(&mut self, sink: EventSink) {
        self.event_sink = Some(sink);
    }

    pub fn set_thread_name(&mut self, name: Option<String>) {
        self.thread_name = name;
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

        // Use streaming path when event_sink is available
        if self.event_sink.is_some() {
            request["stream"] = json!(true);
            tracing::debug!(
                backend = ?self.backend,
                endpoint = "responses",
                request_bytes = json_value_len_bytes(&request)?,
                streaming = true,
                "built openai responses request (streaming)"
            );
            return self.post_streaming_openai_responses(&url, &request).await;
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

    async fn post_streaming_openai_responses(
        &self,
        url: &str,
        body: &Value,
    ) -> Result<ModelTurnResponse> {
        let event_sink = self.event_sink.as_ref().unwrap();
        let thread_name = self.thread_name.clone();
        let mut last_error = anyhow!("No attempts made");
        let request_bytes = json_value_len_bytes(body)?;

        for attempt in 0..3 {
            if attempt > 0 {
                let delay_secs = 1u64 << (attempt - 1);
                tracing::warn!(
                    backend = ?self.backend,
                    endpoint = "responses_stream",
                    attempt = attempt + 1,
                    backoff_secs = delay_secs,
                    "retrying streaming model HTTP request after backoff"
                );
                sleep(Duration::from_secs(delay_secs)).await;
            }

            let attempt_started = Instant::now();
            tracing::debug!(
                backend = ?self.backend,
                endpoint = "responses_stream",
                attempt = attempt + 1,
                request_bytes,
                "starting streaming model HTTP attempt"
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

            if status.is_success() {
                tracing::info!(
                    backend = ?self.backend,
                    endpoint = "responses_stream",
                    attempt = attempt + 1,
                    status = status.as_u16(),
                    request_bytes,
                    latency_ms = attempt_started.elapsed().as_millis() as u64,
                    "streaming model HTTP connected"
                );

                // Read the stream incrementally
                let result = self
                    .read_sse_stream(response, event_sink, &thread_name, url)
                    .await;

                event_sink.emit(AgentEvent::StreamComplete {
                    thread_name: thread_name.clone(),
                });

                return result;
            }

            // Read the error body for non-success status
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "[failed to read body]".to_string());

            if status.as_u16() == 429 || status.is_server_error() {
                tracing::warn!(
                    backend = ?self.backend,
                    endpoint = "responses_stream",
                    attempt = attempt + 1,
                    status = status.as_u16(),
                    request_bytes,
                    latency_ms = attempt_started.elapsed().as_millis() as u64,
                    retryable = true,
                    "streaming model HTTP attempt failed with retryable status"
                );
                last_error = anyhow!(
                    "HTTP {} from {}: {}",
                    status.as_u16(),
                    url,
                    &error_body[..error_body.len().min(500)]
                );
                continue;
            }

            tracing::error!(
                backend = ?self.backend,
                endpoint = "responses_stream",
                attempt = attempt + 1,
                status = status.as_u16(),
                request_bytes,
                latency_ms = attempt_started.elapsed().as_millis() as u64,
                retryable = false,
                "streaming model HTTP attempt failed with non-retryable status"
            );
            return Err(anyhow!(
                "HTTP {} from {}: {}",
                status.as_u16(),
                url,
                &error_body[..error_body.len().min(500)]
            ));
        }

        Err(last_error)
    }

    async fn read_sse_stream(
        &self,
        response: reqwest::Response,
        event_sink: &EventSink,
        thread_name: &Option<String>,
        url: &str,
    ) -> Result<ModelTurnResponse> {
        use futures_util::StreamExt;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut output_items: Vec<(usize, Value)> = Vec::new();
        let mut final_response: Option<Value> = None;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| anyhow!("Stream read error: {}", e))?;
            let chunk_str = String::from_utf8_lossy(&chunk);
            buffer.push_str(&chunk_str);

            // Process complete SSE events (separated by \n\n)
            while let Some(boundary) = buffer.find("\n\n") {
                let event_block = buffer[..boundary].to_string();
                buffer = buffer[boundary + 2..].to_string();

                // Parse the SSE event block
                let mut event_type = String::new();
                let mut data_parts: Vec<String> = Vec::new();

                for line in event_block.lines() {
                    if let Some(value) = line.strip_prefix("event:") {
                        event_type = value.trim().to_string();
                    } else if let Some(value) = line.strip_prefix("data:") {
                        let trimmed = value.trim_start();
                        data_parts.push(trimmed.to_string());
                    }
                }

                if data_parts.is_empty() {
                    continue;
                }

                let data = data_parts.join("\n");
                if data == "[DONE]" {
                    break;
                }

                let event: Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let etype = event_type.as_str();
                let json_type = event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                match etype.is_empty().then_some(json_type).unwrap_or(etype) {
                    "response.output_text.delta" => {
                        if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                            event_sink.emit(AgentEvent::StreamTextDelta {
                                thread_name: thread_name.clone(),
                                text: Some(delta.to_string()),
                            });
                        }
                    }
                    "response.output_item.done" => {
                        if let Some(item) = event.get("item").cloned() {
                            let output_index = event
                                .get("output_index")
                                .and_then(Value::as_u64)
                                .and_then(|i| usize::try_from(i).ok())
                                .unwrap_or(output_items.len());
                            output_items.retain(|(idx, _)| *idx != output_index);
                            output_items.push((output_index, item));
                        }
                    }
                    "response.completed" | "response.done" | "response.incomplete" => {
                        if let Some(resp) = event.get("response").and_then(Value::as_object) {
                            let mut response_value = Value::Object(resp.clone());
                            // If the terminal event has empty output, use accumulated items
                            let output_is_empty = response_value
                                .get("output")
                                .and_then(Value::as_array)
                                .map(Vec::is_empty)
                                .unwrap_or(true);
                            if output_is_empty && !output_items.is_empty() {
                                output_items.sort_by_key(|(idx, _)| *idx);
                                response_value["output"] = Value::Array(
                                    output_items
                                        .iter()
                                        .map(|(_, item)| item.clone())
                                        .collect(),
                                );
                            }
                            final_response = Some(response_value);
                        }
                    }
                    "error" | "response.failed" => {
                        let msg = event
                            .get("error")
                            .and_then(|e| e.get("message"))
                            .and_then(Value::as_str)
                            .or_else(|| event.get("message").and_then(Value::as_str))
                            .unwrap_or("Unknown streaming error");
                        return Err(anyhow!("Streaming error from {}: {}", url, msg));
                    }
                    _ => {}
                }
            }

            if final_response.is_some() {
                break;
            }
        }

        // Parse the final response
        match final_response {
            Some(value) => parse_openai_responses_response(&value, url),
            None => {
                // Fallback: if no terminal event but we have output_items, build response
                if !output_items.is_empty() {
                    output_items.sort_by_key(|(idx, _)| *idx);
                    let constructed = json!({
                        "status": "completed",
                        "output": output_items.iter().map(|(_, item)| item.clone()).collect::<Vec<_>>(),
                        "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}
                    });
                    parse_openai_responses_response(&constructed, url)
                } else {
                    Err(anyhow!(
                        "SSE stream from {} ended without a terminal response event",
                        url
                    ))
                }
            }
        }
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
            event_sink: None,
            thread_name: None,
        }
    }
}
