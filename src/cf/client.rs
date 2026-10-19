use super::{
    cache,
    model::{ApiProblemset, ApiResponse, ApiSubmission, ApiUser},
    CfError, UserInfo,
};
use crate::domain::{Problem, Submission};
use rand::Rng;
use serde::de::DeserializeOwned;
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};

const API_BASE: &str = "https://codeforces.com/api/";
const REQUEST_INTERVAL: Duration = Duration::from_secs(2);
const MAX_RETRIES: usize = 3;
const USER_AGENT: &str = concat!(
    "cfvirtual/",
    env!("CARGO_PKG_VERSION"),
    " (open-source practice tool)"
);

// This is deliberately not a client field: clones and independently constructed
// clients all queue through the same process-wide Tokio mutex.
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::const_new(None);

/// A cloneable, read-only public API client with process-wide request pacing.
#[derive(Clone)]
pub struct CfClient {
    // new() is infallible; preserve a rare TLS/client construction failure as a
    // typed request error instead of panicking or silently changing transports.
    http: Result<reqwest::Client, Arc<reqwest::Error>>,
    base_url: String,
}

impl Default for CfClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CfClient {
    pub fn new() -> Self {
        Self {
            http: Self::http_builder().build().map_err(Arc::new),
            base_url: API_BASE.to_owned(),
        }
    }

    fn http_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
    }

    // Only in crate unit tests; production exposes no URL/environment override.
    #[cfg(test)]
    fn with_base_url(base_url: String) -> Self {
        Self {
            http: Self::http_builder().no_proxy().build().map_err(Arc::new),
            base_url,
        }
    }

    pub async fn user_info(&self, handle: &str) -> Result<UserInfo, CfError> {
        validate_handle(handle)?;
        let users: Vec<ApiUser> = self
            .request("user.info", &[("handles", handle.to_owned())])
            .await?;
        users
            .into_iter()
            .next()
            .map(UserInfo::from)
            .ok_or(CfError::Protocol {
                endpoint: "user.info",
                message: "the API returned no user",
            })
    }

    /// Fetch the latest 10,000 submissions, retaining verdicts and team members.
    /// Solved-problem deduplication belongs to the domain analysis, not this feed.
    pub async fn history(&self, handle: &str) -> Result<Vec<Submission>, CfError> {
        self.submissions(handle, 10_000).await
    }

    /// Fetch the latest 100 submissions for live contest polling.
    pub async fn recent(&self, handle: &str) -> Result<Vec<Submission>, CfError> {
        self.submissions(handle, 100).await
    }

    async fn submissions(&self, handle: &str, count: u32) -> Result<Vec<Submission>, CfError> {
        validate_handle(handle)?;
        let submissions: Vec<ApiSubmission> = self
            .request(
                "user.status",
                &[
                    ("handle", handle.to_owned()),
                    ("from", "1".to_owned()),
                    ("count", count.to_string()),
                ],
            )
            .await?;
        Ok(submissions
            .into_iter()
            .filter_map(ApiSubmission::into_submission)
            .collect())
    }

    /// Read a 24-hour cache at the supplied file path, or refresh from the API.
    /// Valid stale data is returned if refresh fails, including forced refreshes.
    /// Cache-write failures are logged; successfully fetched data remains usable.
    pub async fn problemset(&self, path: &Path, refresh: bool) -> Result<Vec<Problem>, CfError> {
        let cached = match cache::read(path).await {
            Ok(cached) => cached,
            Err(error) => {
                tracing::warn!(%error, "ignoring unreadable Codeforces problemset cache");
                None
            }
        };
        if !refresh {
            if let Some(snapshot) = &cached {
                if snapshot.is_fresh() {
                    return Ok(snapshot.problems.clone());
                }
            }
        }
        let fetched: Result<ApiProblemset, _> = self.request("problemset.problems", &[]).await;
        let problems = match fetched {
            Ok(fetched) => fetched.into_problems(),
            Err(error) => {
                if let Some(snapshot) = cached {
                    tracing::warn!(%error, "using cached Codeforces problemset after API failure");
                    return Ok(snapshot.problems);
                }
                return Err(error);
            }
        };
        if let Err(error) = cache::write(path, &problems).await {
            tracing::warn!(%error, "could not persist Codeforces problemset cache");
        }
        Ok(problems)
    }

    async fn request<T: DeserializeOwned>(
        &self,
        endpoint: &'static str,
        query: &[(&str, String)],
    ) -> Result<T, CfError> {
        let mut retries = 0;
        loop {
            let result = self.request_once(endpoint, query).await;
            if retries < MAX_RETRIES && result.as_ref().is_err_and(CfError::is_rate_limited) {
                retries += 1;
                // The random generator is dropped before await, keeping futures
                // Send. Every retry waits at least five seconds plus jitter.
                let jitter_ms = rand::thread_rng().gen_range(1..=1_000);
                tokio::time::sleep(Duration::from_millis(5_000 + jitter_ms)).await;
            } else {
                return result;
            }
        }
    }

    async fn request_once<T: DeserializeOwned>(
        &self,
        endpoint: &'static str,
        query: &[(&str, String)],
    ) -> Result<T, CfError> {
        let http = self
            .http
            .as_ref()
            .map_err(|error| CfError::ClientInitialization(Arc::clone(error)))?;
        let request = http
            .get(format!("{}{endpoint}", self.base_url))
            .query(query);
        let mut last = LAST_REQUEST.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + REQUEST_INTERVAL).await;
        }
        *last = Some(Instant::now());
        // Keep the lock until send has actually started and received headers.
        // This also prevents scheduling a later request ahead of this one.
        let response = request.send().await;
        drop(last);
        let response = response.map_err(|source| CfError::request(endpoint, source))?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(CfError::Http { endpoint, status });
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|source| CfError::request(endpoint, source))?;
        if !status.is_success() {
            // Codeforces may use HTTP 400 for a normal FAILED API envelope,
            // including invalid handles and call-limit responses.
            if let Ok(failure) = serde_json::from_slice::<ApiResponse<serde_json::Value>>(&bytes) {
                if failure.status == "FAILED" {
                    if let Some(comment) = failure.comment {
                        return Err(CfError::api(endpoint, comment));
                    }
                }
            }
            return Err(CfError::Http { endpoint, status });
        }
        let response: ApiResponse<T> = serde_json::from_slice(&bytes)
            .map_err(|source| CfError::Decode { endpoint, source })?;
        match response.status.as_str() {
            "OK" => response.result.ok_or(CfError::Protocol {
                endpoint,
                message: "OK response is missing result",
            }),
            "FAILED" => Err(CfError::api(
                endpoint,
                response
                    .comment
                    .unwrap_or_else(|| "API failed without an explanation".to_owned()),
            )),
            _ => Err(CfError::Protocol {
                endpoint,
                message: "unrecognized API status",
            }),
        }
    }
}

fn validate_handle(handle: &str) -> Result<(), CfError> {
    if handle.trim().is_empty() {
        Err(CfError::InvalidHandle {
            comment: "handle must not be empty".to_owned(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Request, State},
        http::StatusCode,
        response::{IntoResponse, Response},
        Router,
    };
    use serde_json::{json, Value};
    use std::{collections::VecDeque, sync::Mutex as StdMutex};
    use tokio::{net::TcpListener, sync::MutexGuard, task::JoinHandle};

    static TEST_REQUESTS: Mutex<()> = Mutex::const_new(());

    // Separate test runtimes share the production limiter. Serialize network
    // tests and clear its timestamp only before a new, isolated scenario.
    async fn isolated_requests() -> MutexGuard<'static, ()> {
        let guard = TEST_REQUESTS.lock().await;
        *LAST_REQUEST.lock().await = None;
        guard
    }

    #[derive(Clone, Debug)]
    struct RecordedRequest {
        path: String,
        query: Vec<(String, String)>,
        user_agent: String,
        started_at: Instant,
    }

    struct Reply {
        status: StatusCode,
        body: String,
        delay: Duration,
    }

    impl Reply {
        fn json(status: StatusCode, body: Value) -> Self {
            Self {
                status,
                body: body.to_string(),
                delay: Duration::ZERO,
            }
        }

        fn ok(result: Value) -> Self {
            Self::json(StatusCode::OK, json!({"status": "OK", "result": result}))
        }
    }

    #[derive(Default)]
    struct MockState {
        replies: VecDeque<Reply>,
        requests: Vec<RecordedRequest>,
    }

    struct MockServer {
        state: Arc<StdMutex<MockState>>,
        base_url: String,
        task: JoinHandle<()>,
    }

    impl MockServer {
        async fn start(replies: Vec<Reply>) -> Self {
            let state = Arc::new(StdMutex::new(MockState {
                replies: replies.into(),
                requests: Vec::new(),
            }));
            let router = Router::new()
                .fallback(mock_response)
                .with_state(Arc::clone(&state));
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind mock API");
            let address = listener.local_addr().expect("mock address");
            let task = tokio::spawn(async move {
                axum::serve(listener, router).await.expect("serve mock API");
            });
            Self {
                state,
                base_url: format!("http://{address}/api/"),
                task,
            }
        }

        fn client(&self) -> CfClient {
            CfClient::with_base_url(self.base_url.clone())
        }

        fn requests(&self) -> Vec<RecordedRequest> {
            self.state.lock().expect("mock lock").requests.clone()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn mock_response(
        State(state): State<Arc<StdMutex<MockState>>>,
        request: Request,
    ) -> Response {
        let reply = {
            let mut state = state.lock().expect("mock lock");
            state.requests.push(RecordedRequest {
                path: request.uri().path().to_owned(),
                query: url::form_urlencoded::parse(
                    request.uri().query().unwrap_or_default().as_bytes(),
                )
                .into_owned()
                .collect(),
                user_agent: request
                    .headers()
                    .get("user-agent")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned(),
                started_at: Instant::now(),
            });
            state.replies.pop_front().unwrap_or_else(|| {
                Reply::json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"status": "FAILED", "comment": "unexpected test request"}),
                )
            })
        };
        tokio::time::sleep(reply.delay).await;
        (
            reply.status,
            [("content-type", "application/json")],
            reply.body,
        )
            .into_response()
    }

    fn submission(id: u64, verdict: Value) -> Value {
        json!({
            "id": id,
            "contestId": 7001,
            "creationTimeSeconds": 1700000000,
            "verdict": verdict,
            "problem": {
                "contestId": 7001, "index": "A", "name": "Synthetic sum",
                "rating": 1200, "tags": ["math"]
            },
            "author": {"members": [{"handle": "fixture_alpha"}, {"handle": "fixture_beta"}]}
        })
    }

    fn problemset_result(name: &str) -> Value {
        json!({
            "problems": [
                {"contestId": 7001, "index": "A", "name": name, "rating": 1200, "tags": ["math"]},
                {"contestId": 7002, "index": "A", "name": "Unrated sample", "tags": []},
                {"contestId": 7001, "index": "B", "name": "No statistics", "rating": 1500, "tags": []},
                {"index": "A", "name": "Non-contest sample", "rating": 1200, "tags": []},
                {"contestId": 7003, "index": "A", "name": "April Fools sample", "rating": 1200, "tags": []},
                {"contestId": 7004, "index": "A", "name": "Language sample", "rating": 1200, "tags": ["Kotlin_Heroes"]},
                {"contestId": 7005, "index": "A", "name": "Q# sample", "rating": 1200, "tags": []},
                {"contestId": 7006, "index": "A", "name": "Unknown Language sample", "rating": 1200, "tags": []},
                {"contestId": 7007, "index": "A", "name": "Special sample", "rating": 1200, "tags": ["*special"]}
            ],
            "problemStatistics": [
                {"contestId": 7002, "index": "A", "solvedCount": 4},
                {"contestId": 7001, "index": "A", "solvedCount": 37},
                {"index": "A", "solvedCount": 99}
            ]
        })
    }

    fn assert_problemset(problems: &[Problem], name: &str) {
        assert_eq!(problems.len(), 3);
        assert_eq!(problems[0].key.contest_id, 7001);
        assert_eq!(problems[0].key.index, "A");
        assert_eq!(problems[0].name, name);
        assert_eq!(problems[0].rating, Some(1200));
        assert_eq!(problems[0].tags, ["math"]);
        assert_eq!(problems[0].solved_count, Some(37));
        assert_eq!(problems[1].rating, None);
        assert_eq!(problems[1].solved_count, Some(4));
        assert_eq!(problems[2].solved_count, None);
    }

    #[tokio::test]
    async fn user_info_and_status_parse_camel_case_and_send_exact_queries() {
        let _isolated = isolated_requests().await;
        let mut pending = submission(12, Value::Null);
        pending
            .as_object_mut()
            .expect("submission object")
            .remove("verdict");
        pending["problem"]
            .as_object_mut()
            .expect("problem object")
            .remove("rating");
        pending["problem"]
            .as_object_mut()
            .expect("problem object")
            .remove("contestId");
        let mut non_contest = submission(13, json!("OK"));
        non_contest
            .as_object_mut()
            .expect("submission object")
            .remove("contestId");
        let history = json!([submission(11, json!("OK")), pending, non_contest]);
        let server = MockServer::start(vec![
            Reply::ok(json!([{"handle": "Fixture_Alpha", "rating": 1742}])),
            Reply::ok(json!([{"handle": "fixture_beta"}])),
            Reply::ok(history),
            Reply::ok(json!([submission(14, json!("WRONG_ANSWER"))])),
        ])
        .await;
        let client = server.client();
        assert_eq!(
            client.user_info("fixture_alpha").await.expect("user info"),
            UserInfo {
                handle: "Fixture_Alpha".to_owned(),
                rating: 1742
            }
        );
        assert_eq!(
            client
                .user_info("fixture_beta")
                .await
                .expect("unrated user")
                .rating,
            1200
        );
        let history = client.history("fixture_alpha").await.expect("history");
        assert_eq!(history.len(), 2, "skip only the non-contest submission");
        assert_eq!(history[0].id, 11);
        assert_eq!(history[0].problem.contest_id, 7001);
        assert_eq!(history[0].problem.index, "A");
        assert_eq!(history[0].created_at, 1_700_000_000);
        assert_eq!(history[0].verdict.as_deref(), Some("OK"));
        assert_eq!(history[0].handles, ["fixture_alpha", "fixture_beta"]);
        assert_eq!(history[0].rating, Some(1200));
        assert_eq!(history[0].tags, ["math"]);
        assert_eq!(history[1].verdict, None);
        assert_eq!(history[1].rating, None);
        let recent = client.recent("fixture_alpha").await.expect("recent");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, 14);
        assert_eq!(recent[0].verdict.as_deref(), Some("WRONG_ANSWER"));
        let requests = server.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].path, "/api/user.info");
        assert_eq!(
            requests[0].query,
            [("handles".to_owned(), "fixture_alpha".to_owned())]
        );
        for (request, count) in [(&requests[2], "10000"), (&requests[3], "100")] {
            assert_eq!(request.path, "/api/user.status");
            assert_eq!(
                request.query,
                [
                    ("handle".to_owned(), "fixture_alpha".to_owned()),
                    ("from".to_owned(), "1".to_owned()),
                    ("count".to_owned(), count.to_owned()),
                ]
            );
        }
        assert!(requests
            .iter()
            .all(|request| request.user_agent == USER_AGENT));
    }

    #[tokio::test]
    async fn limiter_is_shared_by_clones_and_independent_clients() {
        let _isolated = isolated_requests().await;
        let server = MockServer::start(
            (0..3)
                .map(|_| Reply::ok(json!([{"handle": "fixture_alpha"}])))
                .collect(),
        )
        .await;
        let first = server.client();
        let cloned = first.clone();
        let independent = server.client();
        let start = Instant::now();
        let (a, b, c) = tokio::join!(
            first.user_info("fixture_alpha"),
            cloned.user_info("fixture_alpha"),
            independent.user_info("fixture_alpha"),
        );
        a.expect("first client");
        b.expect("clone");
        c.expect("independent client");
        assert!(start.elapsed() >= REQUEST_INTERVAL * 2);
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        for pair in requests.windows(2) {
            // The server's arrival timestamps can differ slightly from send
            // starts due to loopback scheduling; the total above is strict.
            assert!(pair[1].started_at - pair[0].started_at >= Duration::from_millis(1990));
        }
    }

    #[tokio::test]
    async fn problemset_cache_survives_restart_refreshes_and_falls_back_when_stale() {
        let _isolated = isolated_requests().await;
        let directory = tempfile::tempdir().expect("temporary cache directory");
        let path = directory.path().join("nested/problemset.json");
        let server = MockServer::start(vec![
            Reply::ok(problemset_result("Original sample")),
            Reply::ok(problemset_result("Refreshed sample")),
            Reply::json(StatusCode::SERVICE_UNAVAILABLE, json!({"status": "FAILED"})),
        ])
        .await;
        {
            let first_process = server.client();
            assert_problemset(
                &first_process
                    .problemset(&path, false)
                    .await
                    .expect("initial fetch"),
                "Original sample",
            );
        }
        let restarted = server.client();
        assert_problemset(
            &restarted
                .problemset(&path, false)
                .await
                .expect("disk cache"),
            "Original sample",
        );
        assert_eq!(
            server.requests().len(),
            1,
            "restart must use the on-disk fresh cache"
        );
        assert_problemset(
            &restarted
                .problemset(&path, true)
                .await
                .expect("forced refresh"),
            "Refreshed sample",
        );
        assert_eq!(server.requests().len(), 2);
        let mut cached: Value =
            serde_json::from_slice(&tokio::fs::read(&path).await.expect("read cache"))
                .expect("cache JSON");
        cached["fetched_at"] = json!(1); // A valid but expired cache, without waiting a day.
        let stale_bytes = serde_json::to_vec(&cached).expect("serialize expired cache");
        tokio::fs::write(&path, &stale_bytes)
            .await
            .expect("expire cache");
        assert_problemset(
            &restarted
                .problemset(&path, false)
                .await
                .expect("stale fallback"),
            "Refreshed sample",
        );
        assert_eq!(server.requests().len(), 3);
        assert_eq!(
            tokio::fs::read(&path).await.expect("preserved cache"),
            stale_bytes
        );
        let mut entries = tokio::fs::read_dir(path.parent().expect("cache parent"))
            .await
            .expect("cache directory");
        assert!(entries.next_entry().await.expect("first entry").is_some());
        assert!(entries
            .next_entry()
            .await
            .expect("no temporary files")
            .is_none());
        assert!(server.requests().iter().all(|request| {
            request.path == "/api/problemset.problems" && request.query.is_empty()
        }));
    }

    #[tokio::test]
    async fn corrupt_cache_is_replaced_and_failed_forced_refresh_keeps_fresh_cache() {
        let _isolated = isolated_requests().await;
        let directory = tempfile::tempdir().expect("cache directory");
        let path = directory.path().join("problemset.json");
        tokio::fs::write(&path, b"incomplete cache")
            .await
            .expect("corrupt cache");
        let server = MockServer::start(vec![
            Reply::ok(problemset_result("Recovered sample")),
            Reply::json(StatusCode::BAD_GATEWAY, json!({"status": "FAILED"})),
        ])
        .await;
        let client = server.client();
        assert_problemset(
            &client
                .problemset(&path, false)
                .await
                .expect("recover cache"),
            "Recovered sample",
        );
        let good_bytes = tokio::fs::read(&path).await.expect("good cache");
        assert_problemset(
            &client
                .problemset(&path, true)
                .await
                .expect("forced refresh fallback"),
            "Recovered sample",
        );
        assert_eq!(
            tokio::fs::read(&path).await.expect("unchanged cache"),
            good_bytes
        );
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn http_and_api_call_limits_retry_with_at_least_five_seconds_between_attempts() {
        let _isolated = isolated_requests().await;
        let server = MockServer::start(vec![
            Reply::json(StatusCode::TOO_MANY_REQUESTS, json!({"status": "FAILED"})),
            Reply::json(
                StatusCode::BAD_REQUEST,
                json!({"status": "FAILED", "comment": "Call limit exceeded"}),
            ),
            Reply::ok(json!([{"handle": "fixture_alpha", "rating": 1300}])),
        ])
        .await;
        assert_eq!(
            server
                .client()
                .user_info("fixture_alpha")
                .await
                .expect("eventual success")
                .rating,
            1300
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        for pair in requests.windows(2) {
            assert!(pair[1].started_at - pair[0].started_at >= Duration::from_secs(5));
            assert_eq!(pair[0].query, pair[1].query);
        }
    }

    #[tokio::test]
    async fn call_limit_retries_stop_after_three_retries() {
        let _isolated = isolated_requests().await;
        let server = MockServer::start(
            (0..4)
                .map(|_| {
                    Reply::json(
                        StatusCode::OK,
                        json!({"status": "FAILED", "comment": "Call limit exceeded"}),
                    )
                })
                .collect(),
        )
        .await;
        let error = server
            .client()
            .history("fixture_alpha")
            .await
            .expect_err("rate limit exhausted");
        assert!(
            matches!(&error, CfError::Api { endpoint: "user.status", comment } if comment.contains("Call limit exceeded"))
        );
        assert!(!error.is_invalid_handle());
        let requests = server.requests();
        assert_eq!(
            requests.len(),
            4,
            "one initial request plus exactly three retries"
        );
        assert!(requests
            .windows(2)
            .all(|pair| pair[1].started_at - pair[0].started_at >= Duration::from_secs(5)));
    }

    #[tokio::test]
    async fn invalid_handle_http_json_and_protocol_errors_are_typed_without_retries() {
        let _isolated = isolated_requests().await;
        let server = MockServer::start(vec![
            Reply::json(StatusCode::SERVICE_UNAVAILABLE, json!({"error": "unavailable"})),
            Reply::json(StatusCode::BAD_REQUEST, json!({"status": "FAILED", "comment": "handles: User with handle fixture_missing not found"})),
            Reply { status: StatusCode::OK, body: "not JSON".to_owned(), delay: Duration::ZERO },
            Reply::json(StatusCode::OK, json!({"status": "OK"})),
            Reply::json(StatusCode::OK, json!({"status": "FAILED", "comment": "Maintenance"})),
        ])
        .await;
        let client = server.client();
        assert!(client
            .user_info("  ")
            .await
            .expect_err("empty handle")
            .is_invalid_handle());
        assert!(server.requests().is_empty());
        // Preserve ordinary HTTP failures, but decode FAILED API envelopes even
        // when Codeforces sends them with HTTP 400.
        assert!(matches!(
            client.user_info("fixture_missing").await,
            Err(CfError::Http {
                status: StatusCode::SERVICE_UNAVAILABLE,
                ..
            })
        ));
        let invalid = client
            .user_info("fixture_missing")
            .await
            .expect_err("invalid API handle");
        assert!(invalid.is_invalid_handle());
        assert!(invalid.to_string().contains("not found"));
        assert!(matches!(
            client.user_info("fixture_alpha").await,
            Err(CfError::Decode { .. })
        ));
        assert!(matches!(
            client.user_info("fixture_alpha").await,
            Err(CfError::Protocol { .. })
        ));
        let directory = tempfile::tempdir().expect("cache directory");
        assert!(matches!(
            client
                .problemset(&directory.path().join("missing.json"), false)
                .await,
            Err(CfError::Api { .. })
        ));
        assert_eq!(
            server.requests().len(),
            5,
            "ordinary failures must not retry"
        );
    }

    #[tokio::test]
    async fn timeouts_and_connection_failures_are_distinct() {
        let _isolated = isolated_requests().await;
        let server = MockServer::start(vec![Reply {
            status: StatusCode::OK,
            body: json!({"status": "OK", "result": [{"handle": "fixture_alpha"}]}).to_string(),
            delay: Duration::from_secs(1),
        }])
        .await;
        let mut client = server.client();
        client.http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(50))
            .build()
            .map_err(Arc::new);
        assert!(matches!(
            client.user_info("fixture_alpha").await,
            Err(CfError::Timeout { .. })
        ));
        assert_eq!(server.requests().len(), 1);
        drop(server);
        assert!(matches!(
            client.user_info("fixture_alpha").await,
            Err(CfError::Transport { .. })
        ));
    }
}
