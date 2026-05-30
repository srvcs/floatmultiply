use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-floatmultiply";
pub const CONCERN: &str = "float arithmetic: a * b";
pub const DEPENDS_ON: &[&str] = &["srvcs-isnumber"];

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub isnumber_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    #[schema(value_type = Object)]
    pub a: Value,
    #[schema(value_type = Object)]
    pub b: Value,
}

#[derive(Serialize, ToSchema)]
pub struct ProductResponse {
    #[schema(value_type = Object)]
    pub a: Value,
    #[schema(value_type = Object)]
    pub b: Value,
    /// The product `a * b` as a floating-point number.
    pub result: f64,
}

/// The single concern: the floating-point product of two real numbers.
///
/// Both integer and fractional inputs are valid — these are float services —
/// and the result is an `f64` that may itself be fractional.
pub fn multiply(a: f64, b: f64) -> f64 {
    a * b
}

fn ok(a: Value, b: Value, result: f64) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "a": a, "b": b, "result": result })),
    )
        .into_response()
}

fn invalid(reason: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": reason })),
    )
        .into_response()
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

/// Forward a dependency's response verbatim (used to propagate `422` for invalid
/// input, so floatmultiply reports the same rejection its dependency did).
fn forward(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, Json(body)).into_response()
}

/// Validate one operand is a number by asking `srvcs-isnumber`, mapping its
/// failures to the response this service should return.
async fn ask_is_number(url: &str, value: &Value, dependency: &str) -> Result<(), Response> {
    match client::call(url, &json!({ "value": value })).await {
        Err(DepError::Unreachable) => Err(degraded(dependency)),
        Ok((200, body)) => {
            let is_number = body.get("result").and_then(Value::as_bool).unwrap_or(false);
            if is_number {
                Ok(())
            } else {
                Err(invalid("value is not a number"))
            }
        }
        // Invalid input propagates from the leaf dependency; forward it.
        Ok((422, body)) => Err(forward(422, body)),
        Ok(_) => Err(degraded(dependency)),
    }
}

/// `POST /` — compute `a * b`.
///
/// Input validation is delegated to `srvcs-isnumber` over HTTP (the single
/// source of truth for "is this a number"), once per operand. Both integer and
/// fractional inputs are accepted and coerced with `as_f64`; the result is an
/// `f64`. If the dependency is unreachable, this service reports itself degraded
/// rather than guessing.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = ProductResponse),
        (status = 422, description = "an operand is not a number"),
        (status = 500, description = "an operand passed validation but is not representable as f64"),
        (status = 503, description = "a dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    // 1. Delegate "is this a number" to srvcs-isnumber, once per operand.
    if let Err(resp) = ask_is_number(&deps.isnumber_url, &req.a, "srvcs-isnumber").await {
        return resp;
    }
    if let Err(resp) = ask_is_number(&deps.isnumber_url, &req.b, "srvcs-isnumber").await {
        return resp;
    }

    // 2. Both operands validated as numbers; coerce to f64 (accepts ints + floats).
    let (Some(a), Some(b)) = (req.a.as_f64(), req.b.as_f64()) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                json!({ "error": "operand validated as a number but is not representable as f64" }),
            ),
        )
            .into_response();
    };

    ok(req.a, req.b, multiply(a, b))
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, ProductResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    fn approx(got: f64, expected: f64) {
        assert!(
            (got - expected).abs() < 1e-9,
            "expected {expected}, got {got}"
        );
    }

    #[test]
    fn product_of_fractions_is_correct() {
        approx(multiply(1.5, 2.0), 3.0);
        approx(multiply(2.5, 4.0), 10.0);
        approx(multiply(0.1, 0.2), 0.02);
        approx(multiply(-1.5, 2.0), -3.0);
    }

    #[test]
    fn product_of_integers_is_correct() {
        approx(multiply(6.0, 7.0), 42.0);
        approx(multiply(-3.0, -4.0), 12.0);
    }

    #[test]
    fn multiply_by_zero_is_zero() {
        approx(multiply(0.0, 12345.6789), 0.0);
        approx(multiply(9.87, 0.0), 0.0);
    }

    #[test]
    fn multiply_by_one_is_identity() {
        approx(multiply(1.0, 8.125), 8.125);
        approx(multiply(8.125, 1.0), 8.125);
    }

    #[tokio::test]
    async fn index_reports_dependency() {
        let Json(info) = index().await;
        assert_eq!(info.service, "srvcs-floatmultiply");
        assert_eq!(info.concern, "float arithmetic: a * b");
        assert_eq!(info.depends_on, vec!["srvcs-isnumber"]);
    }
}
