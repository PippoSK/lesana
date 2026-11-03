//! Retry rejected native Responses requests without replaying model/tool turns.
use axum::body::Bytes;
use chrono::{DateTime, Utc};
use rig::{
    http_client::{
        DynHttpClient, Error, HeaderMap, HttpClientExt, LazyBody, MultipartForm, Request, Response,
        Result, StreamingResponse,
    },
    wasm_compat::WasmCompatSend,
};
use std::time::Duration;

const MAX_RETRIES: usize = 3;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub(super) struct NativeRateLimitClient {
    inner: DynHttpClient,
}

impl NativeRateLimitClient {
    pub fn new(inner: DynHttpClient) -> Self {
        Self { inner }
    }
}

impl HttpClientExt for NativeRateLimitClient {
    fn send<T, U>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<Bytes> + WasmCompatSend,
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        let client = self.inner.clone();
        let request = request.map(Into::<Bytes>::into);
        async move {
            for retries in 0..=MAX_RETRIES {
                match client.send::<Bytes, U>(request.clone()).await {
                    Err(error)
                        if error
                            .non_success_status()
                            .is_some_and(|status| status.as_u16() == 429) =>
                    {
                        let code = safe_error_code(error.non_success_body());
                        let delay = retry_delay(&error, retries, Utc::now());
                        // Log only allowlisted codes and parsed timing, never
                        // arbitrary headers, error bodies, prompts or keys.
                        tracing::warn!(
                            status = 429,
                            provider_error_code = code,
                            attempt = retries + 1,
                            retry_delay_ms = ?delay.map(|value| value.as_millis() as u64),
                            will_retry = delay.is_some(),
                            "Native Responses rate limit"
                        );
                        let Some(delay) = delay else {
                            return Err(error);
                        };
                        // Await in the original future: research/completion
                        // deadlines also cancel this wait and all later retries.
                        tokio::time::sleep(delay).await;
                    }
                    result => return result,
                }
            }
            unreachable!("the final rejected attempt always returns its error")
        }
    }

    fn send_multipart<U>(
        &self,
        request: Request<MultipartForm>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        self.inner.send_multipart(request)
    }

    fn send_streaming<T>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = Result<StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes> + WasmCompatSend,
    {
        // Lesana uses unary Responses calls in all three model phases.
        self.inner.send_streaming(request)
    }
}

pub(super) fn is_quota_failure(body: Option<&str>) -> bool {
    matches!(
        safe_error_code(body),
        "insufficient_quota" | "billing_hard_limit_reached" | "billing_not_active"
    )
}

fn safe_error_code(body: Option<&str>) -> &'static str {
    let value = body.and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok());
    let error = value.as_ref().and_then(|value| value.get("error"));
    let codes = ["code", "type"].map(|field| {
        error
            .and_then(|error| error.get(field))
            .and_then(|value| value.as_str())
    });
    for known in [
        "insufficient_quota",
        "billing_hard_limit_reached",
        "billing_not_active",
        "rate_limit_exceeded",
    ] {
        if codes.contains(&Some(known)) {
            return known;
        }
    }
    "unknown"
}

fn retry_delay(error: &Error, retries: usize, now: DateTime<Utc>) -> Option<Duration> {
    if error.non_success_status()?.as_u16() != 429
        || retries >= MAX_RETRIES
        || is_quota_failure(error.non_success_body())
    {
        return None;
    }
    let delay = error
        .non_success_headers()
        .and_then(|headers| retry_after(headers, now))
        .unwrap_or_else(|| Duration::from_secs(2 << retries));
    // Do not retry sooner than a provider asks, or extend a turn indefinitely.
    (delay <= MAX_RETRY_DELAY).then_some(delay)
}

fn retry_after(headers: &HeaderMap, now: DateTime<Utc>) -> Option<Duration> {
    let number = |name: &str, scale: f64| {
        let value = headers.get(name)?.to_str().ok()?.parse::<f64>().ok()? / scale;
        Duration::try_from_secs_f64(value).ok()
    };
    if let Some(delay) = number("retry-after-ms", 1000.0).or_else(|| number("retry-after", 1.0)) {
        return Some(delay);
    }
    let date = DateTime::parse_from_rfc2822(headers.get("retry-after")?.to_str().ok()?).ok()?;
    Some(
        date.signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::http_client::StatusCode;
    use std::sync::{Arc, Mutex};

    fn failure(status: u16, code: &str, retry_after: Option<&str>) -> Error {
        let mut headers = HeaderMap::new();
        if let Some(value) = retry_after {
            headers.insert("retry-after", value.parse().unwrap());
        }
        Error::non_success_with_details(
            StatusCode::from_u16(status).unwrap(),
            headers,
            serde_json::json!({"error": {"code": code}}).to_string(),
        )
    }

    #[test]
    fn retries_only_rate_limits_with_a_finite_attempt_budget() {
        let error = failure(429, "rate_limit_exceeded", None);
        for retries in 0..3 {
            assert_eq!(
                retry_delay(&error, retries, Utc::now()),
                Some(Duration::from_secs(2 << retries))
            );
        }
        assert!(retry_delay(&error, 3, Utc::now()).is_none());
        for code in [
            "insufficient_quota",
            "billing_hard_limit_reached",
            "billing_not_active",
        ] {
            assert!(retry_delay(&failure(429, code, Some("0")), 0, Utc::now()).is_none());
        }
        for status in [400, 401, 402, 403, 500, 503] {
            assert!(
                retry_delay(
                    &failure(status, "rate_limit_exceeded", Some("0")),
                    0,
                    Utc::now()
                )
                .is_none()
            );
        }
    }

    #[test]
    fn respects_provider_delay_without_exceeding_the_wait_budget() {
        let now = DateTime::parse_from_rfc3339("2026-10-13T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for (header, seconds) in [("0", 0), ("30", 30), ("Tue, 13 Oct 2026 12:01:00 GMT", 60)] {
            assert_eq!(
                retry_delay(&failure(429, "rate_limit_exceeded", Some(header)), 0, now),
                Some(Duration::from_secs(seconds))
            );
        }
        assert!(retry_delay(&failure(429, "rate_limit_exceeded", Some("121")), 0, now).is_none());
        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", "1500".parse().unwrap());
        assert_eq!(
            retry_after(&headers, now),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn logs_only_allowlisted_codes_and_recognizes_quota_in_either_field() {
        assert_eq!(
            safe_error_code(Some(r#"{"error":{"code":"sensitive arbitrary value"}}"#)),
            "unknown"
        );
        assert_eq!(safe_error_code(Some("not JSON")), "unknown");
        assert!(is_quota_failure(Some(
            r#"{"error":{"code":null,"type":"insufficient_quota"}}"#
        )));
        assert!(!is_quota_failure(Some(
            r#"{"error":{"message":"insufficient_quota"}}"#
        )));
    }

    #[derive(Clone)]
    struct RejectingClient {
        requests: Arc<Mutex<Vec<Bytes>>>,
        retry_after: &'static str,
    }

    impl HttpClientExt for RejectingClient {
        fn send<T, U>(
            &self,
            request: Request<T>,
        ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
        where
            T: Into<Bytes> + WasmCompatSend,
            U: From<Bytes> + WasmCompatSend + 'static,
        {
            self.requests
                .lock()
                .unwrap()
                .push(request.into_body().into());
            let delay = self.retry_after;
            async move { Err(failure(429, "rate_limit_exceeded", Some(delay))) }
        }
        fn send_multipart<U>(
            &self,
            _: Request<MultipartForm>,
        ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
        where
            U: From<Bytes> + WasmCompatSend + 'static,
        {
            async { panic!("unexpected multipart request") }
        }
        fn send_streaming<T>(
            &self,
            _: Request<T>,
        ) -> impl Future<Output = Result<StreamingResponse>> + WasmCompatSend
        where
            T: Into<Bytes> + WasmCompatSend,
        {
            async { panic!("unexpected streaming request") }
        }
    }

    #[tokio::test]
    async fn retries_exact_request_bytes_at_most_three_times() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let client = NativeRateLimitClient::new(DynHttpClient::new(RejectingClient {
            requests: requests.clone(),
            retry_after: "0",
        }));
        let body =
            Bytes::from_static(br#"{"reasoning":{"effort":"max"},"max_output_tokens":32000}"#);
        let result = client.send::<_, Bytes>(Request::new(body.clone())).await;
        assert_eq!(
            result.err().unwrap().non_success_status().unwrap().as_u16(),
            429
        );
        assert_eq!(*requests.lock().unwrap(), vec![body; 4]);
    }

    #[tokio::test]
    async fn cancellation_during_backoff_never_sends_a_later_request() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let client = NativeRateLimitClient::new(DynHttpClient::new(RejectingClient {
            requests: requests.clone(),
            retry_after: "60",
        }));
        let operation = client.send::<_, Bytes>(Request::new(Bytes::new()));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), operation)
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}
