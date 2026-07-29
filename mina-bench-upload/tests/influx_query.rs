//! Mock-server tests for the InfluxDB regression-query path.
//!
//! `influx::query::historical_mean` is the other database-touching path
//! (alongside the upload path in `influx_upload.rs`). A `wiremock` server
//! returns a Flux annotated-CSV response and we assert the samples are
//! parsed and averaged. This path previously used the typed `query::<T>`
//! helper, which panics on a projection that omits `_field` -- exactly
//! what our `keep(["_value","_time"])` produces -- so these tests lock in
//! the `query_raw` behaviour against both a populated and an empty result.

use mina_bench_upload::config::InfluxConfig;
use mina_bench_upload::influx::query::historical_mean;
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

/// A Flux annotated-CSV result for a `keep(["_value","_time"])`
/// projection: two samples, no `_field` column (the shape that broke the
/// typed query helper).
const TWO_SAMPLE_CSV: &str = "\
#datatype,string,long,dateTime:RFC3339,double\r
#group,false,false,false,false\r
#default,_result,,,\r
,result,table,_time,_value\r
,,0,2024-01-15T00:00:00Z,52268\r
,,0,2024-01-14T00:00:00Z,52270\r
\r
";

#[tokio::test]
async fn historical_mean_parses_samples_without_field_column() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TWO_SAMPLE_CSV))
        .mount(&server)
        .await;

    let hist = historical_mean(
        &cfg_for(&server),
        &["develop".to_string()],
        "Zkapp_command.t",
        "heap words",
        10,
    )
    .await
    .expect("query should succeed")
    .expect("two samples means Some");

    assert_eq!(hist.samples_found, 2);
    // (52268 + 52270) / 2
    assert!((hist.mean - 52269.0).abs() < 1e-9, "mean was {}", hist.mean);
}

#[tokio::test]
async fn historical_mean_is_none_on_empty_result() {
    let server = MockServer::start().await;
    // No matching series -> InfluxDB returns an empty body. This must be
    // "no history" (None), not a panic or an error.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .mount(&server)
        .await;

    let hist = historical_mean(
        &cfg_for(&server),
        &["develop".to_string()],
        "Nonexistent.t",
        "heap words",
        10,
    )
    .await
    .expect("empty query should still succeed");
    assert!(hist.is_none(), "empty result must be None, got {hist:?}");
}

#[tokio::test]
async fn historical_mean_queries_the_union_of_branches() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TWO_SAMPLE_CSV))
        .mount(&server)
        .await;

    let branches = [
        "develop".to_string(),
        "compatible".to_string(),
        "master".to_string(),
    ];
    let hist = historical_mean(
        &cfg_for(&server),
        &branches,
        "Zkapp_command.t",
        "heap words",
        10,
    )
    .await
    .expect("query should succeed")
    .expect("two samples means Some");
    assert_eq!(hist.samples_found, 2);

    // The Flux query must filter on all three branches, ORed together.
    let reqs = server
        .received_requests()
        .await
        .expect("request recording enabled");
    let body = String::from_utf8_lossy(&reqs[0].body);
    for b in ["develop", "compatible", "master"] {
        assert!(body.contains(b), "query missing branch {b}; body: {body}");
    }
    assert!(body.contains(" or "), "branches must be ORed; body: {body}");
}
