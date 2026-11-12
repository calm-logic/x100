//! Bodies stream through the proxy in both directions without being held whole, and swapping
//! and redaction still work where a reference or a value is split between writes.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use common::*;
use http::{Request, Response};
use http_body_util::BodyExt;
use tokio::sync::{Notify, oneshot};
use x100_core::credential::UseOutcome;
use x100_core::driver::Purpose;

const CHUNK: usize = 64 * 1024;

fn filler(n: usize, seed: u8) -> Bytes {
    Bytes::from(
        (0..n)
            .map(|i| b'a' + ((i as u8).wrapping_add(seed) % 26))
            .collect::<Vec<u8>>(),
    )
}

#[tokio::test]
async fn a_streamed_response_of_several_megabytes_arrives_as_it_is_sent_with_a_split_value_redacted()
 {
    let mut s = setup();
    let release = Arc::new(Notify::new());
    let gate = Arc::clone(&release);
    let server = TestServer::https(Some(s.ca.server_config(&[ALLOWED])), move |_req| {
        let gate = Arc::clone(&gate);
        async move {
            let (tx, body) = channel_body();
            tokio::spawn(async move {
                tx.send(Bytes::from_static(b"data: first event\n\n"))
                    .await
                    .unwrap();
                // Nothing more until the client has seen the first event: a proxy that buffered
                // the body whole would never deliver it.
                gate.notified().await;
                let (head, tail) = VALUE.split_at(11);
                tx.send(Bytes::from(format!("data: {{\"leak\":\"{head}")))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                tx.send(Bytes::from(format!("{tail}\"}}\n\n")))
                    .await
                    .unwrap();
                for i in 0..128 {
                    tx.send(filler(CHUNK, i)).await.unwrap();
                }
                tx.send(Bytes::from_static(b"data: [DONE]\n\n"))
                    .await
                    .unwrap();
            });
            let mut response = Response::new(body);
            response
                .headers_mut()
                .insert("content-type", "text/event-stream".parse().unwrap());
            response
        }
    })
    .await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[], &[])
        .await
        .unwrap();
    let request = Request::get("/v1/messages")
        .header("host", ALLOWED)
        .body(empty())
        .unwrap();
    let response = https_response(&route, &s.authority, ALLOWED, server.port, request).await;
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&first[..], b"data: first event\n\n");
    release.notify_one();
    let mut rest = Vec::new();
    let mut frames = 1;
    while let Some(frame) = body.frame().await {
        rest.extend_from_slice(&frame.unwrap().into_data().unwrap());
        frames += 1;
    }
    assert!(frames > 10, "the body came in {frames} pieces");
    let mut expected = format!("data: {{\"leak\":\"{}\"}}\n\n", s.reference).into_bytes();
    for i in 0..128 {
        expected.extend_from_slice(&filler(CHUNK, i));
    }
    expected.extend_from_slice(b"data: [DONE]\n\n");
    assert_eq!(rest.len(), expected.len());
    assert!(rest == expected, "the stream changed in transit");
    assert_no_leak("the streamed response", &rest);
    assert_eq!(s.report().await.uses, vec![]);
}

#[tokio::test]
async fn a_streamed_upload_with_a_reference_split_across_writes_is_swapped_as_it_goes() {
    let mut s = setup();
    let (first_seen, got_first) = oneshot::channel::<()>();
    let first_seen = Arc::new(Mutex::new(Some(first_seen)));
    let received = Arc::new(Mutex::new((Vec::new(), 0usize, None::<String>)));
    let record = Arc::clone(&received);
    let server = TestServer::https(
        Some(s.ca.server_config(&[ALLOWED])),
        move |req: Request<hyper::body::Incoming>| {
            let first_seen = Arc::clone(&first_seen);
            let record = Arc::clone(&record);
            async move {
                let encoding = req
                    .headers()
                    .get("transfer-encoding")
                    .map(|v| v.to_str().unwrap().to_owned());
                let mut body = req.into_body();
                while let Some(frame) = body.frame().await {
                    let data = frame.unwrap().into_data().unwrap();
                    if let Some(tx) = first_seen.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    let mut r = record.lock().unwrap();
                    if r.0.len() < 4096 {
                        r.0.extend_from_slice(&data);
                    }
                    r.1 += data.len();
                    r.2 = encoding.clone();
                }
                respond(200, "stored")
            }
        },
    )
    .await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[], &[])
        .await
        .unwrap();
    let (tx, body) = channel_body();
    let request = Request::put("/upload")
        .header("host", ALLOWED)
        .body(body)
        .unwrap();
    let reference = s.reference.to_string();
    let (head, tail) = reference.split_at(13);
    let start = format!("{{\"key\":\"{head}");
    let client = tokio::spawn({
        let (route_authority, port) = (s.authority.clone(), server.port);
        async move { https_via(&route, &route_authority, ALLOWED, port, request).await }
    });
    tx.send(Bytes::from(start.clone())).await.unwrap();
    // The server sees the start of the body before the client has written the rest.
    tokio::time::timeout(Duration::from_secs(10), got_first)
        .await
        .expect("the first bytes reached the server while the upload was still open")
        .unwrap();
    tx.send(Bytes::from(format!("{tail}\"}}\n"))).await.unwrap();
    let total_filler = 256 * CHUNK;
    for i in 0..256 {
        tx.send(filler(CHUNK, i as u8)).await.unwrap();
    }
    drop(tx);
    let seen = client.await.unwrap();
    assert_eq!((seen.status, seen.text().as_str()), (200, "stored"));
    let (head_bytes, total, encoding) = received.lock().unwrap().clone();
    let expected_head = format!("{{\"key\":\"{VALUE}\"}}\n");
    assert!(
        head_bytes.starts_with(expected_head.as_bytes()),
        "{:?}",
        String::from_utf8_lossy(&head_bytes[..80])
    );
    assert_eq!(total, expected_head.len() + total_filler);
    assert_eq!(encoding.as_deref(), Some("chunked"));
    assert_eq!(
        s.report().await.uses,
        vec![(s.reference, UseOutcome::Injected)]
    );
}

#[tokio::test]
async fn a_large_upload_with_a_declared_length_keeps_its_length_and_passes_unchanged() {
    let mut s = setup();
    let received = Arc::new(Mutex::new((0usize, None::<String>, false)));
    let record = Arc::clone(&received);
    let reference = s.reference.to_string();
    let server = TestServer::https(
        Some(s.ca.server_config(&[ALLOWED])),
        move |req: Request<hyper::body::Incoming>| {
            let record = Arc::clone(&record);
            let reference = reference.clone();
            async move {
                let length = req
                    .headers()
                    .get("content-length")
                    .map(|v| v.to_str().unwrap().to_owned());
                let mut body = req.into_body();
                let mut total = 0;
                let mut saw_reference = false;
                while let Some(frame) = body.frame().await {
                    let data = frame.unwrap().into_data().unwrap();
                    saw_reference |= memchr::memmem::find(&data, reference.as_bytes()).is_some();
                    total += data.len();
                }
                *record.lock().unwrap() = (total, length, saw_reference);
                respond(201, "")
            }
        },
    )
    .await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[], &[])
        .await
        .unwrap();
    let size = 6 << 20;
    let mut data = format!("{} ", s.reference).into_bytes();
    data.resize(size, b'x');
    let request = Request::put("/artifact")
        .header("host", ALLOWED)
        .header("content-length", size)
        .body(full(data))
        .unwrap();
    let seen = https_via(&route, &s.authority, ALLOWED, server.port, request).await;
    assert_eq!(seen.status, 201);
    assert_eq!(
        *received.lock().unwrap(),
        (size, Some(size.to_string()), true)
    );
    assert_eq!(s.report().await.uses, vec![]);
}
