//! Bounded owner-routed streaming measurements. SSE chunks are not token counts.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, &'static str>;
const ERROR: &str = "streaming-evaluation-unavailable";
const CASES: &[(&str, &str, Option<&str>)] = &[
    (
        "arithmetic",
        "What is 17 times 23? Reply with only the integer.",
        Some("391"),
    ),
    (
        "json",
        "Return only this JSON object with no markdown: {\"ok\":true,\"count\":3}",
        Some("{\"ok\":true,\"count\":3}"),
    ),
    (
        "code",
        "Write a Python function merge_intervals(intervals) that merges overlapping closed intervals. Include a short explanation and three examples. Do not use external libraries.",
        None,
    ),
];

#[derive(Default)]
struct Stream {
    first_sse_ms: Option<f64>,
    first_output_ms: Option<f64>,
    first_answer_ms: Option<f64>,
    last_output_ms: Option<f64>,
    content: String,
    reasoning: String,
    usage: Option<Value>,
    finish_reason: Option<String>,
    done: bool,
    output_events: usize,
}
impl Stream {
    fn event(&mut self, data: &str, elapsed_ms: f64) -> Result<()> {
        if self.done {
            return Err(ERROR);
        }
        self.first_sse_ms.get_or_insert(elapsed_ms);
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let value: Value = serde_json::from_str(data).map_err(|_| ERROR)?;
        if value.get("error").is_some() {
            return Err(ERROR);
        }
        if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
            self.usage = Some(usage.clone());
        }
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(ERROR)?;
        if choices.len() > 1 {
            return Err(ERROR);
        }
        if let Some(choice) = choices.first() {
            let delta = choice.get("delta").ok_or(ERROR)?;
            let content = delta.get("content").and_then(Value::as_str).unwrap_or("");
            let reasoning = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !content.is_empty() || !reasoning.is_empty() {
                self.first_output_ms.get_or_insert(elapsed_ms);
                self.last_output_ms = Some(elapsed_ms);
                self.output_events += 1;
            }
            if !content.is_empty() {
                self.first_answer_ms.get_or_insert(elapsed_ms);
            }
            self.content.push_str(content);
            self.reasoning.push_str(reasoning);
            if self.content.len() + self.reasoning.len() > 1_048_576 {
                return Err(ERROR);
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.into());
            }
        }
        Ok(())
    }
    fn finish(self, elapsed_ms: f64, expected: Option<&str>) -> Result<Value> {
        if !self.done || self.first_output_ms.is_none() || self.finish_reason.is_none() {
            return Err(ERROR);
        }
        let tokens = self
            .usage
            .as_ref()
            .and_then(|u| u.get("completion_tokens"))
            .and_then(Value::as_u64)
            .filter(|n| *n > 0);
        let output_window_ms = self.last_output_ms.unwrap() - self.first_output_ms.unwrap();
        let exact_match = expected.map(|expected| {
            if expected.starts_with('{') {
                serde_json::from_str::<Value>(self.content.trim()).ok()
                    == serde_json::from_str::<Value>(expected).ok()
            } else {
                self.content.trim() == expected
            }
        });
        Ok(json!({
            "first_sse_ms": self.first_sse_ms,
            "ttft_text_ms": self.first_output_ms,
            "first_answer_text_ms": self.first_answer_ms,
            "elapsed_ms": elapsed_ms,
            "output_window_ms": output_window_ms,
            "output_events": self.output_events,
            "usage": self.usage,
            "completion_tokens": tokens,
            "end_to_end_tokens_per_second": tokens.map(|n| n as f64 * 1000.0 / elapsed_ms),
            "decode_window_tokens_per_second_estimate": tokens.filter(|_| output_window_ms > 0.0).map(|n| n as f64 * 1000.0 / output_window_ms),
            "finish_reason": self.finish_reason,
            "exact_match": exact_match,
            "content": self.content,
            "reasoning": self.reasoning,
        }))
    }
}

fn request(
    client: &reqwest::blocking::Client,
    port: u16,
    model: &str,
    prompt: &str,
    expected: Option<&str>,
    deadline: Instant,
) -> Result<Value> {
    let timeout = deadline
        .checked_duration_since(Instant::now())
        .ok_or(ERROR)?
        .min(Duration::from_secs(180));
    let start = Instant::now();
    let response = client.post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .timeout(timeout)
        .json(&json!({"model":model,"messages":[{"role":"user","content":prompt}],"stream":true,
            "stream_options":{"include_usage":true},"max_tokens":512,"temperature":1.0,"top_p":0.95,"top_k":20,"min_p":0.0}))
        .send().map_err(|_| ERROR)?;
    if response.status() != reqwest::StatusCode::OK
        || !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        return Err(ERROR);
    }
    let mut reader = BufReader::new(response);
    let mut stream = Stream::default();
    let mut event = String::new();
    let mut bytes = 0usize;
    loop {
        let mut line = String::new();
        let n = (&mut reader)
            .take(65537)
            .read_line(&mut line)
            .map_err(|_| ERROR)?;
        bytes = bytes.checked_add(n).ok_or(ERROR)?;
        if n == 0 || n > 65536 || bytes > 4_194_304 || Instant::now() >= deadline {
            return Err(ERROR);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() && !event.is_empty() {
            stream.event(&event, start.elapsed().as_secs_f64() * 1000.0)?;
            event.clear();
            if stream.done {
                break;
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            if !event.is_empty() {
                event.push('\n');
            }
            event.push_str(data.strip_prefix(' ').unwrap_or(data));
            if event.len() > 65536 {
                return Err(ERROR);
            }
        }
    }
    stream.finish(start.elapsed().as_secs_f64() * 1000.0, expected)
}

fn retain(directory: &std::path::Path, index: usize, sample: &Value) -> Result<()> {
    super::owner::private_file(
        &directory.join(format!("evaluation-sample-{index:02}.json")),
        &serde_json::to_vec_pretty(sample).map_err(|_| ERROR)?,
    )
}

pub(super) fn run(
    port: u16,
    model: &str,
    deadline: Instant,
    directory: &std::path::Path,
) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| ERROR)?;
    let mut samples = Vec::new();
    // First request is retained as cold-after-load evidence, excluded from warm summaries.
    let mut cold = request(&client, port, model, CASES[0].1, CASES[0].2, deadline)?;
    cold["case"] = json!(CASES[0].0);
    cold["phase"] = json!("first-after-model-discovery");
    retain(directory, samples.len(), &cold)?;
    samples.push(cold);
    for repetition in 0..3 {
        for &(name, prompt, expected) in CASES {
            let mut sample = request(&client, port, model, prompt, expected, deadline)?;
            sample["case"] = json!(name);
            sample["phase"] = json!("warm");
            sample["repetition"] = json!(repetition);
            retain(directory, samples.len(), &sample)?;
            samples.push(sample);
        }
    }
    Ok(
        json!({"schema":"m8s.streaming-evaluation/v1","model":model,"concurrency":1,"max_tokens":512,
        "temperature":1.0,"top_p":0.95,"top_k":20,"samples":samples,
        "measurement_notes":"TTFT is first nonempty content or reasoning delta at the owner client; first SSE may only be a role event. Token counts come only from server usage, never chunk count. Decode-window rate is an estimate because streaming chunks can batch tokens; end-to-end rate includes request latency. Warm requests may reuse prompt cache. Exact-match tasks are smoke tests; code output is retained, not executed or scored."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn role_and_reasoning_are_distinct_from_answer_and_token_usage() {
        let mut s = Stream::default();
        s.event(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#, 10.0)
            .unwrap();
        s.event(
            r#"{"choices":[{"delta":{"reasoning_content":"think"}}]}"#,
            30.0,
        )
        .unwrap();
        s.event(
            r#"{"choices":[{"delta":{"content":"391"},"finish_reason":"stop"}]}"#,
            80.0,
        )
        .unwrap();
        s.event(r#"{"choices":[],"usage":{"completion_tokens":7}}"#, 90.0)
            .unwrap();
        s.event("[DONE]", 100.0).unwrap();
        let v = s.finish(100.0, Some("391")).unwrap();
        assert_eq!(v["first_sse_ms"], 10.0);
        assert_eq!(v["ttft_text_ms"], 30.0);
        assert_eq!(v["first_answer_text_ms"], 80.0);
        assert_eq!(v["output_events"], 2);
        assert_eq!(v["completion_tokens"], 7);
        assert_eq!(v["exact_match"], true);
        assert_eq!(v["end_to_end_tokens_per_second"], 70.0);
    }
    #[test]
    fn missing_usage_never_turns_chunks_into_tokens_and_truncation_refuses() {
        let mut s = Stream::default();
        s.event(r#"{"choices":[{"delta":{"content":"many tokens in one chunk"},"finish_reason":"length"}]}"#,20.0).unwrap();
        assert!(s.finish(30.0, None).is_err());
        let mut s = Stream::default();
        s.event(
            r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"stop"}]}"#,
            20.0,
        )
        .unwrap();
        s.event("[DONE]", 30.0).unwrap();
        let v = s.finish(30.0, None).unwrap();
        assert!(v["completion_tokens"].is_null());
        assert!(v["end_to_end_tokens_per_second"].is_null());
    }
}
