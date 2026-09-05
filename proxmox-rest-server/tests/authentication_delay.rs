use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;
use http::request::Parts;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use proxmox_http::Body;
use proxmox_rest_server::ApiConfig;
use proxmox_router::{
    ApiHandler, ApiMethod, ApiResponseFuture, Permission, Router, RpcEnvironment,
    RpcEnvironmentType, http_err,
};
use proxmox_schema::{IntegerSchema, ObjectSchema};
use serde_json::Value;
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;

fn reject_login(
    _parts: Parts,
    params: Value,
    _info: &'static ApiMethod,
    _rpcenv: Box<dyn RpcEnvironment>,
) -> ApiResponseFuture {
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(params["check_ms"].as_u64().unwrap())).await;
        Err(http_err!(UNAUTHORIZED, "authentication failed"))
    })
}

const METHOD: ApiMethod = ApiMethod::new(
    &ApiHandler::AsyncHttpBodyParameters(&reject_login),
    &ObjectSchema::new(
        "A login attempt.",
        &[(
            "check_ms",
            false,
            &IntegerSchema::new("Authentication runtime in milliseconds.").schema(),
        )],
    ),
)
.protected(true)
.access(None, &Permission::World);
static ROUTER: Router = Router::new().post(&METHOD);

async fn response_time(check_ms: u64) -> Result<Duration, Error> {
    let config =
        Arc::new(ApiConfig::new(".", RpcEnvironmentType::PRIVILEGED).default_api2_handler(&ROUTER));
    let dispatched = Arc::new(Notify::new());
    let handler_dispatched = Arc::clone(&dispatched);
    let (client_io, server_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        hyper::server::conn::http1::Builder::new()
            .serve_connection(
                TokioIo::new(server_io),
                service_fn(move |request| {
                    handler_dispatched.notify_one();
                    let config = Arc::clone(&config);
                    async move {
                        config
                            .handle_request(request, &"127.0.0.1:1234".parse().unwrap(), None)
                            .await
                    }
                }),
            )
            .await
    });
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(client_io)).await?;
    let client = tokio::spawn(connection);
    let (body_tx, body_rx) = mpsc::channel::<Result<Vec<u8>, Error>>(1);
    let body = format!("{{\"check_ms\":{check_ms}}}");
    let request = http::Request::builder()
        .method("POST")
        .uri("/api2/json")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("content-length", body.len())
        .body(Body::wrap_stream(ReceiverStream::new(body_rx)))?;
    body_tx.send(Ok(body.as_bytes()[..1].to_vec())).await?;
    let response = tokio::spawn(async move { sender.send_request(request).await });
    dispatched.notified().await;

    // The client can exhaust a request-arrival deadline before supplying the login credentials.
    tokio::time::advance(Duration::from_secs(4)).await;
    let sent = Instant::now();
    body_tx.send(Ok(body.as_bytes()[1..].to_vec())).await?;
    drop(body_tx);
    let response = response.await??;
    let elapsed = sent.elapsed();
    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);

    client.abort();
    server.abort();
    let _ = client.await;
    let _ = server.await;
    Ok(elapsed)
}

#[tokio::test(start_paused = true)]
async fn a_slow_body_does_not_expose_authentication_runtime() -> Result<(), Error> {
    for check_ms in [20, 220] {
        let elapsed = response_time(check_ms).await?;
        assert!(elapsed >= Duration::from_secs(3), "{elapsed:?}");
        assert!(elapsed <= Duration::from_millis(3002), "{elapsed:?}");
    }
    Ok(())
}
