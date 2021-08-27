use axum::body::{Bytes, Full};
use axum::response::IntoResponse;
use axum::{extract::Path, handler::get, response::Html, routing::BoxRoute, Json, Router};
use chrono::format::ParseError;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use hyper::StatusCode;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() {
    // Set the RUST_LOG, if it hasn't been explicitly defined
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "timestamp_microservice=debug,tower_http=debug")
    }
    tracing_subscriber::fmt::init();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::info!("listening on {}", addr);

    axum::Server::bind(&addr)
        .serve(app().into_make_service())
        .await
        .unwrap();
}

/// Having an app function makes it easy to call it from test
fn app() -> Router<BoxRoute> {
    Router::new()
        .route("/", get(hello_handler))
        .route("/api", get(now_handler))
        .route("/api/:date", get(date_handler))
        .layer(TraceLayer::new_for_http())
        .boxed()
}

async fn hello_handler() -> Html<&'static str> {
    Html("<h1>Hello World!</h1>")
}

async fn date_handler(Path(mut date): Path<String>) -> Result<Json<Value>, AppError> {
    tracing::info!("Provided date is {}", date);
    let timestamp = date.parse::<i64>();
    if timestamp.is_ok() {
        let timestamp = timestamp.unwrap();
        let ndt = NaiveDateTime::from_timestamp(timestamp, 0);
        date = ndt.format("%Y-%m-%d").to_string();
        tracing::debug!(
            "We converted from the original timestamp {} to the following date {}",
            timestamp,
            date
        );
    }

    let date: NaiveDate = date.parse()?;
    let date = DateTime::<Utc>::from_utc(date.and_hms(0, 0, 0), Utc);

    tracing::debug!("Converted date is {}", date);
    Ok(Json(json!({
        "unix": date.timestamp(),
        "utc": date.to_rfc2822(),
    })))
}

async fn now_handler() -> Result<Json<Value>, AppError> {
    let utc: DateTime<Utc> = Utc::now();
    Ok(Json(json!({
        "unix": utc.timestamp(),
        "utc": utc.to_rfc2822(),
    })))
}

struct AppError;

impl From<ParseError> for AppError {
    fn from(error: ParseError) -> Self {
        tracing::error!("Error while parsing the date: {}", error);
        AppError
    }
}

impl IntoResponse for AppError {
    type Body = Full<Bytes>;
    type BodyError = Infallible;

    fn into_response(self) -> hyper::Response<Self::Body> {
        let status = StatusCode::UNPROCESSABLE_ENTITY;
        let body = Json(json!({
            "error": "Invalid Date"
        }));

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn hello_world() {
        let app = app();

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

        assert_eq!(&body[..], b"<h1>Hello World!</h1>");
    }

    #[tokio::test]
    async fn not_found() {
        let app = app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/not-found")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn valid_date_string() {
        let app = app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/2016-12-25")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            body,
            json!({
                "unix": 1482624000,
                "utc": "Sun, 25 Dec 2016 00:00:00 +0000"
            })
        );
    }
    // A request to /api/1451001600 should return { unix: 1451001600000, utc: "Fri, 25 Dec 2015 00:00:00 GMT" }
    #[tokio::test]
    async fn timestamp() {
        let app = app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/1451001600")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            body,
            json!({
                "unix": 1451001600,
                "utc": "Fri, 25 Dec 2015 00:00:00 +0000"
            })
        );
    }

    // If the input date string is invalid, the api returns an object having the structure { error : "Invalid Date" }
    #[tokio::test]
    async fn invalid_date() {
        let app = app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/this-is-not-a-date")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            body,
            json!({
                "error": "Invalid Date"
            })
        );
    }

    // An empty date parameter should return the current time in a JSON object with a unix key
    // Note: this test is fragile as it's comparing equally the two responses.
    // A more sound way would be to assert approximately as, due to latecy, the times may differ.
    #[tokio::test]
    async fn empty_param() {
        let app = app();
        let now: DateTime<Utc> = Utc::now();
        let response = app
            .oneshot(Request::builder().uri("/api").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            body,
            json!({
                "unix": now.timestamp(),
                "utc": now.to_rfc2822(),
            })
        );
    }
}
