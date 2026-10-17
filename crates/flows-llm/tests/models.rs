//! `list_models` against a local one-shot HTTP server.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use flows_llm::list_models;

/// Serve one reply; returns the base URL and a handle yielding the raw request.
fn serve(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let h = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while !String::from_utf8_lossy(&buf).contains("\r\n\r\n") {
            let n = sock.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        sock.write_all(reply.as_bytes()).unwrap();
        String::from_utf8_lossy(&buf).to_string()
    });
    (base, h)
}

const SECS: Duration = Duration::from_secs(5);

#[tokio::test]
async fn lists_ids_and_sends_key() {
    let (base, h) = serve(
        "200 OK",
        r#"{"object":"list","data":[{"id":"qwen3"},{"id":"other"}]}"#,
    );
    let ids = list_models(&base, "sk-test", SECS).await.unwrap();
    assert_eq!(ids, ["qwen3", "other"]);
    let req = h.join().unwrap();
    assert!(req.starts_with("GET /v1/models "), "{req}");
    assert!(
        req.to_ascii_lowercase()
            .contains("authorization: bearer sk-test"),
        "{req}"
    );
}

#[tokio::test]
async fn no_key_no_auth_header() {
    let (base, h) = serve("200 OK", r#"{"data":[]}"#);
    assert!(list_models(&base, "", SECS).await.unwrap().is_empty());
    assert!(!h
        .join()
        .unwrap()
        .to_ascii_lowercase()
        .contains("authorization"));
}

#[tokio::test]
async fn http_error_and_bad_body() {
    let (base, _h) = serve("401 Unauthorized", "{}");
    assert_eq!(list_models(&base, "", SECS).await.unwrap_err(), "HTTP 401");
    let (base, _h) = serve("200 OK", r#"{"models":[]}"#);
    assert!(list_models(&base, "", SECS)
        .await
        .unwrap_err()
        .contains("no `data` list"));
}

#[tokio::test]
async fn refused_connection() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let e = list_models(&format!("http://127.0.0.1:{port}/v1"), "", SECS)
        .await
        .unwrap_err();
    assert!(e.contains("unreachable"), "{e}");
}

#[tokio::test]
async fn silent_server_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    // Accept but never answer.
    let _h = std::thread::spawn(move || {
        let (_sock, _) = listener.accept().unwrap();
        std::thread::sleep(Duration::from_secs(3));
    });
    let e = list_models(&base, "", Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(e.contains("no answer within 1s"), "{e}");
}
