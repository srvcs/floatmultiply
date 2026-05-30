use axum::body::Body;
use axum::extract::Json as ExtractJson;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use srvcs_floatmultiply::{api::Deps, health, router, telemetry};
use tower::ServiceExt;

/// Spin up a mock dependency that answers `POST /` with a fixed status + body,
/// and return its base URL. Lets us test orchestration without the real fleet.
async fn spawn_mock(status: StatusCode, body: Value) -> String {
    let app = AxumRouter::new().route(
        "/",
        post(move || {
            let body = body.clone();
            async move { (status, Json(body)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// A *computing* mock of srvcs-isnumber: it reads the request body and actually
/// decides whether `value` is a JSON number, returning the real verdict in
/// `result`. This genuinely exercises floatmultiply's validation branch (number
/// -> proceed, non-number -> 422) rather than rubber-stamping a fixed answer.
async fn spawn_isnumber() -> String {
    async fn handler(ExtractJson(req): ExtractJson<Value>) -> Json<Value> {
        let value = req.get("value").cloned().unwrap_or(Value::Null);
        let is_number = value.is_number();
        Json(json!({ "value": value, "result": is_number }))
    }
    let app = AxumRouter::new().route("/", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn app(isnumber_url: &str) -> axum::Router {
    router(
        telemetry::metrics_handle_for_tests(),
        Deps {
            isnumber_url: isnumber_url.to_string(),
        },
    )
}

async fn eval(isnumber_url: &str, a: Value, b: Value) -> (StatusCode, Value) {
    let res = app(isnumber_url)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "a": a, "b": b }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Assert a JSON `result` field is approximately `expected` (never exact float
/// equality).
fn assert_result_approx(body: &Value, expected: f64) {
    let got = body["result"].as_f64().expect("result is an f64");
    assert!(
        (got - expected).abs() < 1e-9,
        "expected {expected}, got {got}"
    );
}

// A base URL with nothing listening — exercises the degraded path.
const DEAD_URL: &str = "http://127.0.0.1:1";

async fn status_of(uri: &str) -> StatusCode {
    app(DEAD_URL)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn index_ok() {
    assert_eq!(status_of("/").await, StatusCode::OK);
}

#[tokio::test]
async fn healthz_ok() {
    assert_eq!(status_of("/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn readyz_reflects_state() {
    health::set_ready(true);
    assert_eq!(status_of("/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn openapi_ok() {
    assert_eq!(status_of("/openapi.json").await, StatusCode::OK);
}

#[tokio::test]
async fn product_of_fractions_is_correct() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(2.5), json!(4.0)).await;
    assert_eq!(status, StatusCode::OK);
    assert_result_approx(&body, 10.0);
}

#[tokio::test]
async fn product_with_fractional_result() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(0.1), json!(0.2)).await;
    assert_eq!(status, StatusCode::OK);
    // 0.1 * 0.2 == 0.02, but not exactly representable: compare approximately.
    assert_result_approx(&body, 0.02);
}

#[tokio::test]
async fn product_of_integers_is_accepted_and_correct() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(6), json!(7)).await;
    assert_eq!(status, StatusCode::OK);
    assert_result_approx(&body, 42.0);
}

#[tokio::test]
async fn mixed_integer_and_float_operands() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(3), json!(1.5)).await;
    assert_eq!(status, StatusCode::OK);
    assert_result_approx(&body, 4.5);
    assert_eq!(body["a"], 3);
    assert_eq!(body["b"], 1.5);
}

#[tokio::test]
async fn product_with_negative_operand() {
    let isnumber = spawn_isnumber().await;
    let (status, body) = eval(&isnumber, json!(-2.5), json!(4.0)).await;
    assert_eq!(status, StatusCode::OK);
    assert_result_approx(&body, -10.0);
}

#[tokio::test]
async fn rejects_non_number_first_operand() {
    // The computing mock genuinely reports "nope" is not a number -> 422.
    let isnumber = spawn_isnumber().await;
    let (status, _) = eval(&isnumber, json!("nope"), json!(2.0)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rejects_non_number_second_operand() {
    let isnumber = spawn_isnumber().await;
    let (status, _) = eval(&isnumber, json!(2.0), json!(null)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rejects_when_isnumber_says_false() {
    // Fixed-answer mock asserting the false branch maps to 422.
    let isnumber = spawn_mock(StatusCode::OK, json!({ "result": false })).await;
    let (status, _) = eval(&isnumber, json!(2.5), json!(4.0)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn degrades_when_isnumber_is_unreachable() {
    let (status, body) = eval(DEAD_URL, json!(2.5), json!(4.0)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-isnumber");
}
