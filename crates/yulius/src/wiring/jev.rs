//! TypeSafe's Jev (`jev-1.13.0`, <https://docs.typesafe.ai/api>) as the task-intake
//! [`ChoiceEvaluator`] (SPEC-002 ADR-16).
//!
//! `POST https://api.typesafe.ai/v1/systemone` with `Authorization: Bearer <key>` and
//! `{"model": "jev-1.13.0", "state": …, "questions": {<id>: {"type": "choice", "instructions",
//! "criteria": {<option>: <description>}}}}`. The answer must name the exact model, answer
//! exactly the asked question with an option, give a confidence and a complete probability
//! distribution over exactly the asked options (each 0–1, summing to 1 within the provider's
//! hundredth rounding) and report non-negative token usage; anything else is a typed failure
//! naming the failed check. One request per call: no retries, no redirects, one deadline over
//! connect, request and the whole body, bounded request and response. The key is read from an
//! owner-only file and never appears in a log, an error, a request body or a debug string.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use yulius_app::model::Secret;
use yulius_app::ports::intake::{ChoiceEvaluator, JevCheck, JevConnector, JevFailure};
use yulius_domain::intake::{ChoiceAnswer, ChoiceQuestion, JEV_MODEL};

/// The one endpoint.
pub const JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// Largest request body (below the provider's per-question context).
pub const MAX_REQUEST_BYTES: usize = 28 * 1024;
/// Largest answer body read.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// Upper bound of any deadline a caller passes.
pub const MAX_DEADLINE: Duration = Duration::from_secs(10);
/// Largest key file read.
const MAX_KEY_FILE_BYTES: u64 = 8 * 1024;

/// The Jev API key. Formatting never shows it.
pub struct JevKey(String);

impl std::fmt::Debug for JevKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JevKey(<redacted>)")
    }
}

impl JevKey {
    /// Validates a key: 8–4096 printable ASCII characters without spaces.
    ///
    /// # Errors
    /// A message without the key.
    pub fn new(text: &str) -> Result<Self, String> {
        let key = text.trim();
        if (8..=4096).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_graphic()) {
            Ok(Self(key.to_owned()))
        } else {
            Err("the Jev key must be 8-4096 printable characters without spaces".into())
        }
    }
}

/// Reads an owner-only secret file: a regular file readable by its owner only (mode 0600 or
/// stricter), at most 8 KiB read, the rule of every Yulius secret file (`*_FILE` settings).
///
/// # Errors
/// A message naming the path and the problem, never the content.
pub fn read_owner_only_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let shown = path.display();
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a regular file"));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(format!(
            "{shown} is readable by other users (mode {mode:o}); make it 0600"
        ));
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_KEY_FILE_BYTES).read_to_string(&mut text))
        .map_err(|e| format!("cannot read {shown}: {e}"))?;
    Ok(text)
}

/// Reads the key from `path`: an owner-only file ([`read_owner_only_file`]).
///
/// # Errors
/// A message naming the path and the problem, never the key.
pub fn read_key_file(path: &Path) -> Result<JevKey, String> {
    let text = read_owner_only_file(path)?;
    JevKey::new(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads a secret from an owner-only file (the same rules as the Jev key file): for
/// `YULIUS_UPDATE_TOKEN_FILE`.
///
/// # Errors
/// A message naming the path and the problem, never the secret.
pub fn read_secret_file(path: &Path) -> Result<Secret, String> {
    read_key_file(path).map(|key| Secret(key.0))
}

/// The Jev client.
pub struct JevClient {
    http: reqwest::Client,
    key: JevKey,
    endpoint: String,
}

impl std::fmt::Debug for JevClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JevClient")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl JevClient {
    /// A client for the fixed HTTPS endpoint.
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn new(key: JevKey) -> Result<Self, String> {
        Self::build(key, JEV_ENDPOINT.to_owned(), true)
    }

    fn build(key: JevKey, endpoint: String, https_only: bool) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("yulius/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(https_only)
            .timeout(MAX_DEADLINE)
            .build()
            .map_err(|e| format!("Jev client setup failed: {e}"))?;
        Ok(Self {
            http,
            key,
            endpoint,
        })
    }

    /// The request body for one question, refused before any request when it is empty,
    /// too large or contains the key.
    fn request_body(&self, state: &str, question: &ChoiceQuestion) -> Result<Vec<u8>, JevFailure> {
        if state.trim().is_empty() {
            return Err(JevFailure::Input("input_shape"));
        }
        let criteria: Map<String, Value> = question
            .options
            .iter()
            .map(|o| (o.key.to_owned(), Value::String(o.description.to_owned())))
            .collect();
        let mut questions = Map::new();
        questions.insert(
            question.id.to_owned(),
            json!({"type": "choice", "instructions": question.instructions, "criteria": criteria}),
        );
        let body = json!({"model": JEV_MODEL, "state": state, "questions": questions});
        let bytes = serde_json::to_vec(&body).map_err(|_| JevFailure::Input("input_shape"))?;
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(JevFailure::Input("request_bytes"));
        }
        let text = String::from_utf8_lossy(&bytes);
        let escaped = serde_json::to_string(&self.key.0).unwrap_or_default();
        if text.contains(&self.key.0) || text.contains(escaped.trim_matches('"')) {
            return Err(JevFailure::Input("credential"));
        }
        Ok(bytes)
    }

    async fn exchange(&self, body: Vec<u8>) -> Result<Vec<u8>, JevFailure> {
        let mut response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.key.0)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| JevFailure::Unavailable)?;
        if !response.status().is_success() {
            return Err(JevFailure::Unavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| JevFailure::Invalid("body"))?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(JevFailure::Invalid("body"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

/// Live Jev returns hundredth-rounded probabilities (for example 0.81 + 0.04 + 0.14): accept a
/// sum within 0.001 of 1, or within 0.01 when every value is a whole hundredth. Never
/// renormalized.
fn valid_total(values: &[f64]) -> bool {
    let error = (values.iter().sum::<f64>() - 1.0).abs();
    error <= 0.001
        || error <= 0.01 + 1e-12
            && values
                .iter()
                .all(|v| (v * 100.0 - (v * 100.0).round()).abs() < 1e-9)
}

fn unit(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

/// Checks an answer body against the question (SPEC-002 ADR-16 contract).
///
/// # Errors
/// [`JevFailure::Invalid`] with the name of the failed check.
pub fn parse_answer(bytes: &[u8], question: &ChoiceQuestion) -> Result<ChoiceAnswer, JevFailure> {
    let body: Value = serde_json::from_slice(bytes).map_err(|_| JevFailure::Invalid("json"))?;
    let object = body.as_object().ok_or(JevFailure::Invalid("shape"))?;
    if object.get("model").and_then(Value::as_str) != Some(JEV_MODEL) {
        return Err(JevFailure::Invalid("model"));
    }
    let usage = object
        .get("usage")
        .and_then(Value::as_object)
        .ok_or(JevFailure::Invalid("usage"))?;
    for field in ["input_tokens", "output_tokens"] {
        if usage.get(field).and_then(Value::as_u64).is_none() {
            return Err(JevFailure::Invalid("usage"));
        }
    }
    let answers = object
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(JevFailure::Invalid("shape"))?;
    if answers.len() != 1 {
        return Err(JevFailure::Invalid("answer_set"));
    }
    let answer = answers
        .get(question.id)
        .and_then(Value::as_object)
        .ok_or(JevFailure::Invalid("answer_set"))?;
    if answer.get("type").and_then(Value::as_str) != Some("choice") {
        return Err(JevFailure::Invalid("shape"));
    }
    let choice = answer
        .get("choice")
        .and_then(Value::as_str)
        .ok_or(JevFailure::Invalid("shape"))?;
    if !question.has_option(choice) {
        return Err(JevFailure::Invalid("option_set"));
    }
    let confidence = answer
        .get("confidence")
        .and_then(unit)
        .ok_or(JevFailure::Invalid("confidence"))?;
    let given = answer
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or(JevFailure::Invalid("probabilities"))?;
    if given.len() != question.options.len() {
        return Err(JevFailure::Invalid("option_set"));
    }
    let mut probabilities = Vec::with_capacity(question.options.len());
    for option in question.options {
        let value = given
            .get(option.key)
            .ok_or(JevFailure::Invalid("option_set"))?;
        let p = unit(value).ok_or(JevFailure::Invalid("probabilities"))?;
        probabilities.push((option.key.to_owned(), p));
    }
    let values: Vec<f64> = probabilities.iter().map(|(_, p)| *p).collect();
    if !valid_total(&values) {
        return Err(JevFailure::Invalid("probability_sum"));
    }
    Ok(ChoiceAnswer {
        choice: choice.to_owned(),
        confidence,
        probabilities,
    })
}

#[async_trait]
impl ChoiceEvaluator for JevClient {
    async fn evaluate(
        &self,
        state: &str,
        question: &ChoiceQuestion<'_>,
        deadline: Duration,
    ) -> Result<ChoiceAnswer, JevFailure> {
        let body = self.request_body(state, question)?;
        let bytes = tokio::time::timeout(deadline.min(MAX_DEADLINE), self.exchange(body))
            .await
            .map_err(|_| JevFailure::Timeout)??;
        parse_answer(&bytes, question)
    }
}

/// The state of a test request: a short split question with an obvious answer.
const TEST_STATE: &str = "REQUEST\nRename the Save button on the settings page to Apply.";

impl JevClient {
    /// One small request (SPEC-001 ADM-002 "test the key"): the split question about a one-line
    /// request. The HTTP status tells a refused key from an unreachable service.
    pub async fn check(&self) -> JevCheck {
        use yulius_domain::intake::SPLIT_QUESTION;
        let Ok(body) = self.request_body(TEST_STATE, &SPLIT_QUESTION) else {
            return JevCheck::Invalid;
        };
        let send = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.key.0)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body)
            .send();
        let response = match tokio::time::timeout(MAX_DEADLINE, send).await {
            Err(_) => return JevCheck::Timeout,
            Ok(Err(err)) if err.is_timeout() => return JevCheck::Timeout,
            Ok(Err(_)) => return JevCheck::Unreachable,
            Ok(Ok(response)) => response,
        };
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return JevCheck::Refused,
            _ => return JevCheck::Unreachable,
        }
        let Ok(Ok(bytes)) = tokio::time::timeout(MAX_DEADLINE, response.bytes()).await else {
            return JevCheck::Timeout;
        };
        if bytes.len() > MAX_RESPONSE_BYTES {
            return JevCheck::Invalid;
        }
        match parse_answer(&bytes, &SPLIT_QUESTION) {
            Ok(_) => JevCheck::Ok,
            Err(_) => JevCheck::Invalid,
        }
    }
}

/// Builds [`JevClient`]s from keys a superadmin saves (SPEC-001 ADM-002).
#[derive(Debug, Clone)]
pub struct JevKeys {
    endpoint: String,
    https_only: bool,
}

impl Default for JevKeys {
    fn default() -> Self {
        Self {
            endpoint: JEV_ENDPOINT.to_owned(),
            https_only: true,
        }
    }
}

impl JevKeys {
    /// Keys for another endpoint (tests).
    #[must_use]
    pub fn with_endpoint(endpoint: String) -> Self {
        Self {
            endpoint,
            https_only: false,
        }
    }

    fn client(&self, key: &Secret) -> Result<JevClient, String> {
        JevClient::build(
            JevKey::new(key.expose())?,
            self.endpoint.clone(),
            self.https_only,
        )
    }
}

#[async_trait]
impl JevConnector for JevKeys {
    fn connect(&self, key: &Secret) -> Result<Arc<dyn ChoiceEvaluator>, String> {
        Ok(Arc::new(self.client(key)?))
    }

    async fn check(&self, key: &Secret) -> JevCheck {
        match self.client(key) {
            Ok(client) => client.check().await,
            Err(_) => JevCheck::Invalid,
        }
    }
}

/// The key of `YULIUS_JEV_API_KEY_FILE`: `Ok(None)` when unset, `Err` (a message without the
/// key) when set but unusable.
///
/// # Errors
/// Why the file cannot be used.
pub fn config_key(path: Option<&Path>) -> Result<Option<Secret>, String> {
    match path {
        None => Ok(None),
        Some(path) => read_key_file(path).map(|key| Some(Secret(key.0))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use yulius_domain::intake::{SCOPE_QUESTION, SPLIT_QUESTION};

    use super::*;

    const KEY: &str = "synthetic-jev-key-0001";
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn valid() -> Value {
        json!({"model": "jev-1.13.0", "answers": {"split": {"type": "choice", "choice": "several",
            "confidence": 0.9, "probabilities": {"one": 0.04, "several": 0.96}}},
            "usage": {"input_tokens": 41, "output_tokens": 8}})
    }

    /// A one-shot HTTP server answering every request with `status` and `body` (after
    /// `delay`), recording request heads and bodies.
    struct Fake {
        url: String,
        calls: Arc<AtomicUsize>,
        seen: Arc<std::sync::Mutex<Vec<String>>>,
    }

    async fn fake(status: u16, body: String, delay: Duration) -> Result<Fake, std::io::Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/v1/systemone", listener.local_addr()?);
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (c, s) = (calls.clone(), seen.clone());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                c.fetch_add(1, Ordering::SeqCst);
                let mut buf = vec![0u8; 64 * 1024];
                let mut got = Vec::new();
                // Read the head and the body (Content-Length).
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).to_string();
                    if let Some(at) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if got.len() >= at + 4 + length {
                            break;
                        }
                    }
                }
                if let Ok(mut seen) = s.lock() {
                    seen.push(String::from_utf8_lossy(&got).to_string());
                }
                tokio::time::sleep(delay).await;
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        Ok(Fake { url, calls, seen })
    }

    fn client(url: &str) -> Result<JevClient, String> {
        JevClient::build(JevKey::new(KEY)?, url.to_owned(), false)
    }

    #[tokio::test]
    async fn one_pinned_request_returns_the_checked_answer() -> TestResult {
        let server = fake(200, valid().to_string(), Duration::ZERO).await?;
        let jev = client(&server.url)?;
        let answer = jev
            .evaluate("REQUEST\nbuild it", &SPLIT_QUESTION, Duration::from_secs(5))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(answer.choice, "several");
        assert_eq!(answer.probability("several"), 0.96);
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
        let seen = server.seen.lock().map_err(|_| "lock")?.join("\n");
        assert!(
            seen.contains(&format!("authorization: Bearer {KEY}"))
                || seen.contains(&format!("Authorization: Bearer {KEY}"))
        );
        let body: Value = serde_json::from_str(seen.split("\r\n\r\n").nth(1).ok_or("no body")?)?;
        assert_eq!(body["model"], "jev-1.13.0");
        assert_eq!(body["questions"]["split"]["type"], "choice");
        assert_eq!(
            body["questions"]["split"]["criteria"]
                .as_object()
                .map(Map::len),
            Some(2)
        );
        assert!(!format!("{jev:?}").contains(KEY));
        Ok(())
    }

    #[test]
    fn invalid_answers_name_the_failed_check() {
        type Mutate = fn(&mut Value);
        let cases: &[(&str, Mutate)] = &[
            ("model", |v| v["model"] = json!("jev-latest")),
            ("usage", |v| v["usage"]["input_tokens"] = json!(-1)),
            ("usage", |v| {
                v.as_object_mut().map(|o| o.remove("usage"));
            }),
            ("answer_set", |v| {
                v["answers"].as_object_mut().map(|o| o.remove("split"));
            }),
            ("answer_set", |v| {
                v["answers"]["extra"] = v["answers"]["split"].clone()
            }),
            ("option_set", |v| {
                v["answers"]["split"]["choice"] = json!("maybe")
            }),
            ("shape", |v| v["answers"]["split"]["type"] = json!("score")),
            ("confidence", |v| {
                v["answers"]["split"]["confidence"] = json!(-0.1)
            }),
            ("probabilities", |v| {
                v["answers"]["split"]["probabilities"]["several"] = json!(1.1);
            }),
            ("option_set", |v| {
                v["answers"]["split"]["probabilities"]
                    .as_object_mut()
                    .map(|o| o.remove("one"));
            }),
            ("option_set", |v| {
                v["answers"]["split"]["probabilities"]["extra"] = json!(0.0);
            }),
            ("probability_sum", |v| {
                v["answers"]["split"]["probabilities"]["several"] = json!(0.2);
            }),
        ];
        for (reason, mutate) in cases {
            let mut value = valid();
            mutate(&mut value);
            let bytes = value.to_string().into_bytes();
            assert_eq!(
                parse_answer(&bytes, &SPLIT_QUESTION),
                Err(JevFailure::Invalid(reason)),
                "{value}"
            );
        }
        assert_eq!(
            parse_answer(b"not json", &SPLIT_QUESTION),
            Err(JevFailure::Invalid("json"))
        );
        assert_eq!(
            parse_answer(b"[]", &SPLIT_QUESTION),
            Err(JevFailure::Invalid("shape"))
        );
    }

    #[test]
    fn hundredth_rounding_is_kept_and_disagreeing_choices_survive() -> Result<(), String> {
        let mut value = json!({"model": "jev-1.13.0", "answers": {"scope": {"type": "choice",
            "choice": "context", "confidence": 0.7, "probabilities": {"context": 0.81,
            "new_scope": 0.04, "drops_scope": 0.14, "redirect": 0.0}}},
            "usage": {"input_tokens": 1, "output_tokens": 1}});
        let answer = parse_answer(value.to_string().as_bytes(), &SCOPE_QUESTION)
            .map_err(|e| e.to_string())?;
        assert_eq!(answer.probability("context"), 0.81, "never renormalized");
        value["answers"]["scope"]["probabilities"]["context"] = json!(0.805);
        assert_eq!(
            parse_answer(value.to_string().as_bytes(), &SCOPE_QUESTION),
            Err(JevFailure::Invalid("probability_sum"))
        );
        // A typed choice that disagrees with the distribution is kept for the caller, whose
        // threshold treats it as uncertain.
        value["answers"]["scope"]["probabilities"]["context"] = json!(0.81);
        value["answers"]["scope"]["choice"] = json!("redirect");
        let answer = parse_answer(value.to_string().as_bytes(), &SCOPE_QUESTION)
            .map_err(|e| e.to_string())?;
        assert_eq!(answer.confident(0.5), None);
        Ok(())
    }

    #[tokio::test]
    async fn http_errors_are_unavailable_with_exactly_one_request() -> TestResult {
        for status in [302, 400, 401, 422, 429, 500, 529] {
            let server = fake(status, format!("{{\"echo\":\"{KEY}\"}}"), Duration::ZERO).await?;
            let result = client(&server.url)?
                .evaluate("state", &SPLIT_QUESTION, Duration::from_secs(5))
                .await;
            assert_eq!(result, Err(JevFailure::Unavailable), "HTTP {status}");
            assert!(!format!("{result:?}").contains(KEY));
            assert_eq!(
                server.calls.load(Ordering::SeqCst),
                1,
                "no retry after {status}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_slow_answer_times_out_within_the_deadline() -> TestResult {
        let server = fake(200, valid().to_string(), Duration::from_secs(5)).await?;
        // Building the client (TLS roots, connection pool) is not part of the deadline: time only
        // the call, and bound it by the fake's 5 s answer, which a working deadline never waits for.
        let jev = client(&server.url)?;
        let started = std::time::Instant::now();
        let result = jev
            .evaluate("state", &SPLIT_QUESTION, Duration::from_millis(200))
            .await;
        assert_eq!(result, Err(JevFailure::Timeout));
        assert!(started.elapsed() < Duration::from_secs(3));
        Ok(())
    }

    #[tokio::test]
    async fn oversized_answers_and_bad_input_fail_without_reading_or_sending() -> TestResult {
        let huge = format!("{{\"pad\":\"{}\"}}", "x".repeat(MAX_RESPONSE_BYTES));
        let server = fake(200, huge, Duration::ZERO).await?;
        let jev = client(&server.url)?;
        assert_eq!(
            jev.evaluate("state", &SPLIT_QUESTION, Duration::from_secs(5))
                .await,
            Err(JevFailure::Invalid("body"))
        );
        let calls = server.calls.load(Ordering::SeqCst);
        for (state, reason) in [
            ("  ", "input_shape"),
            (&*format!("leaked {KEY} here"), "credential"),
            (&*"x".repeat(MAX_REQUEST_BYTES), "request_bytes"),
        ] {
            assert_eq!(
                jev.evaluate(state, &SPLIT_QUESTION, Duration::from_secs(5))
                    .await,
                Err(JevFailure::Input(reason))
            );
        }
        assert_eq!(
            server.calls.load(Ordering::SeqCst),
            calls,
            "refused input sent nothing"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_test_request_tells_a_refused_key_from_an_unreachable_service() -> TestResult {
        let keys = |url: &str| JevKeys::with_endpoint(url.to_owned());
        let key = Secret(KEY.into());
        let ok = fake(200, valid().to_string(), Duration::ZERO).await?;
        assert_eq!(keys(&ok.url).check(&key).await, JevCheck::Ok);
        assert_eq!(ok.calls.load(Ordering::SeqCst), 1, "one cheap request");
        let refused = fake(401, "{}".into(), Duration::ZERO).await?;
        assert_eq!(keys(&refused.url).check(&key).await, JevCheck::Refused);
        let down = fake(503, "{}".into(), Duration::ZERO).await?;
        assert_eq!(keys(&down.url).check(&key).await, JevCheck::Unreachable);
        let odd = fake(200, "{\"model\": \"other\"}".into(), Duration::ZERO).await?;
        assert_eq!(keys(&odd.url).check(&key).await, JevCheck::Invalid);
        assert!(keys(&ok.url).connect(&Secret("short".into())).is_err());
        let evaluator = keys(&ok.url).connect(&key).map_err(|e| e.to_string())?;
        let answer = evaluator
            .evaluate("REQUEST\nx", &SPLIT_QUESTION, Duration::from_secs(5))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(answer.choice, "several");
        Ok(())
    }

    #[test]
    fn key_files_must_be_owner_only_and_keys_never_print() -> TestResult {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("jev.key");
        std::fs::write(&path, format!("{KEY}\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        let open = read_key_file(&path).map(|_| ()).err().unwrap_or_default();
        assert!(open.contains("0600"), "{open}");
        assert!(!open.contains(KEY));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let key = read_key_file(&path)?;
        assert!(!format!("{key:?}").contains(KEY));
        assert!(read_key_file(&dir.path().join("missing")).is_err());
        std::fs::write(&path, "short")?;
        assert!(read_key_file(&path).is_err());
        Ok(())
    }
}
