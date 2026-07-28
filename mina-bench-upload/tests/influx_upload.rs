//! Mock-server tests for the InfluxDB upload path.
//!
//! `influx::upload` is the one path the unit and `cli_smoke` tests can't
//! reach without a database. Here a `wiremock` server stands in for
//! InfluxDB v2: we drive a real upload at it and assert on the request it
//! actually received (endpoint, auth, line-protocol body) and on how the
//! function handles a server error.

use mina_bench_upload::config::InfluxConfig;
use mina_bench_upload::influx;
use mina_bench_upload::parse::{BenchmarkRecord, FieldValue, TAG_CATEGORY, TAG_GITBRANCH};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cfg_for(server: &MockServer) -> InfluxConfig {
    InfluxConfig {
        host: server.uri(),
        token: "test-token".into(),
        org: "test-org".into(),
        bucket: "test-bucket".into(),
    }
}

fn sample_records() -> Vec<BenchmarkRecord> {
    vec![BenchmarkRecord::new("archive_memory_bench")
        .with_tag(TAG_CATEGORY, "archive")
        .with_tag(TAG_GITBRANCH, "develop")
        .with_field("pg_backend_rss_peak_kib", FieldValue::Int(161624))
        .with_field("blocks_per_sec", FieldValue::Float(20.87))]
}

#[tokio::test]
async fn upload_posts_line_protocol_with_auth() {
    let server = MockServer::start().await;
    // InfluxDB returns 204 No Content on a successful write. Match any
    // POST so the test asserts the endpoint from the captured request
    // rather than hard-coding the client's URL shape.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let n = influx::upload(&sample_records(), &cfg_for(&server))
        .await
        .expect("upload should succeed against a 204 server");
    assert_eq!(n, 1, "one record uploaded");

    let requests = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(requests.len(), 1, "exactly one write request");
    let req = &requests[0];

    // Hits the v2 write endpoint...
    assert!(
        req.url.path().contains("write"),
        "expected the v2 write endpoint, got path {}",
        req.url.path()
    );
    // ...carries the token...
    let auth = req
        .headers
        .get("Authorization")
        .expect("Authorization header")
        .to_str()
        .unwrap();
    assert!(auth.contains("test-token"), "auth header was {auth:?}");

    // ...and the body is line protocol for our record: measurement,
    // both tags, the integer field (with the `i` suffix) and the float.
    let body = String::from_utf8_lossy(&req.body);
    assert!(body.contains("archive_memory_bench"), "body: {body}");
    assert!(body.contains("category=archive"), "body: {body}");
    assert!(body.contains("gitbranch=develop"), "body: {body}");
    assert!(
        body.contains("pg_backend_rss_peak_kib=161624i"),
        "integer field should keep the i suffix; body: {body}"
    );
    assert!(body.contains("blocks_per_sec=20.87"), "body: {body}");
}

#[tokio::test]
async fn upload_errors_on_server_5xx() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let err = influx::upload(&sample_records(), &cfg_for(&server))
        .await
        .expect_err("a 5xx write must surface as an error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("write") || msg.to_lowercase().contains("influx"),
        "error should mention the failed write; got: {msg}"
    );
}

#[tokio::test]
async fn upload_of_empty_records_makes_no_request() {
    let server = MockServer::start().await;
    // No mock mounted: any request would 404 and fail the upload. The
    // empty-input short-circuit must not touch the network at all.
    let n = influx::upload(&[], &cfg_for(&server))
        .await
        .expect("empty upload is a no-op");
    assert_eq!(n, 0);
    let requests = server.received_requests().await.unwrap();
    assert!(requests.is_empty(), "empty upload must send nothing");
}
