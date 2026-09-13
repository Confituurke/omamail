//! Native, pooled HTTP to the fixed LNemail origin. Errors never include response data.
use serde_json::Value;
use std::{sync::OnceLock, time::Duration};

const ORIGIN: &str = "https://lnemail.net/api/v1/";
const MAX_INPUT: usize = 64 * 1024;
const MAX_RESPONSE: usize = 16 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(20);
static CLIENT: OnceLock<Result<reqwest::Client, &'static str>> = OnceLock::new();

fn client() -> Result<&'static reqwest::Client, &'static str> {
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .https_only(true)
                .hickory_dns(true)
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
                .timeout(DEADLINE)
                .build()
                .map_err(|_| "lnemail_http_failed")
        })
        .as_ref()
        .map_err(|error| *error)
}

fn valid(value: &str) -> Result<(), &'static str> {
    if value.len() > MAX_INPUT || value.bytes().any(|b| b < 32 || b == 127) {
        return Err("lnemail_invalid_input");
    }
    Ok(())
}

/// Path entries are individual components, never an arbitrary URL or slash path.
fn url(path: &[&str]) -> Result<String, &'static str> {
    if path.is_empty() || path.len() > 8 {
        return Err("lnemail_invalid_input");
    }
    let mut out = String::from(ORIGIN);
    for (index, part) in path.iter().enumerate() {
        valid(part)?;
        if part.is_empty() || *part == "." || *part == ".." {
            return Err("lnemail_invalid_input");
        }
        if index != 0 {
            out.push('/');
        }
        out.push_str(part);
    }
    if out.len() > MAX_INPUT {
        return Err("lnemail_invalid_input");
    }
    Ok(out)
}

fn authorization(token: &str) -> Result<Option<reqwest::header::HeaderValue>, &'static str> {
    if token.is_empty() {
        return Ok(None);
    }
    valid(token)?;
    let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "lnemail_invalid_input")?;
    value.set_sensitive(true);
    Ok(Some(value))
}

fn http_error(error: reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "lnemail_timeout"
    } else {
        "lnemail_http_failed"
    }
}

async fn execute(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: String,
    body: Option<String>,
    authorization: Option<reqwest::header::HeaderValue>,
) -> Result<Value, &'static str> {
    tokio::time::timeout(DEADLINE, async {
        let mut builder = client.request(method.clone(), &url);
        if let Some(body) = body {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
        } else if matches!(method, reqwest::Method::POST | reqwest::Method::DELETE) {
            // A bodyless POST/DELETE still needs explicit framing: some
            // gateways reject a request with no Content-Length with HTTP 411.
            builder = builder.header(reqwest::header::CONTENT_LENGTH, "0");
        }
        if let Some(authorization) = authorization {
            builder = builder.header(reqwest::header::AUTHORIZATION, authorization);
        }
        let mut response = builder.send().await.map_err(http_error)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("lnemail_unauthorized");
        }
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                403 => "lnemail_forbidden",
                404 => "lnemail_not_found",
                409 => "lnemail_conflict",
                411 => "lnemail_length_required",
                422 => "lnemail_invalid_input",
                429 => "lnemail_rate_limited",
                _ => "lnemail_http_failed",
            });
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE as u64)
        {
            return Err("lnemail_response_too_large");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(http_error)? {
            if chunk.len() > MAX_RESPONSE - bytes.len() {
                return Err("lnemail_response_too_large");
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Ok(serde_json::json!({}));
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "lnemail_invalid_response")?;
        if !value.is_object() {
            return Err("lnemail_invalid_response");
        }
        Ok(value)
    })
    .await
    .map_err(|_| "lnemail_timeout")?
}

/// `token` is empty for the unauthenticated account-creation and payment-status endpoints.
pub async fn get(path: &[&str], token: &str) -> Result<Value, &'static str> {
    let url = url(path)?;
    let authorization = authorization(token)?;
    execute(client()?, reqwest::Method::GET, url, None, authorization).await
}

pub async fn post(path: &[&str], body: &Value, token: &str) -> Result<Value, &'static str> {
    let url = url(path)?;
    let authorization = authorization(token)?;
    let encoded = serde_json::to_string(body).map_err(|_| "lnemail_invalid_input")?;
    if encoded.len() > 16 * 1024 * 1024 {
        return Err("lnemail_invalid_input");
    }
    execute(
        client()?,
        reqwest::Method::POST,
        url,
        Some(encoded),
        authorization,
    )
    .await
}

pub async fn delete(path: &[&str], body: Option<&Value>, token: &str) -> Result<Value, &'static str> {
    let url = url(path)?;
    let authorization = authorization(token)?;
    let encoded = body
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| "lnemail_invalid_input")?;
    if encoded.as_ref().is_some_and(|body| body.len() > MAX_INPUT) {
        return Err("lnemail_invalid_input");
    }
    execute(
        client()?,
        reqwest::Method::DELETE,
        url,
        encoded,
        authorization,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_stays_on_the_fixed_lnemail_origin() {
        assert_eq!(url(&["emails"]).unwrap(), "https://lnemail.net/api/v1/emails");
        assert_eq!(
            url(&["emails", "abc-123"]).unwrap(),
            "https://lnemail.net/api/v1/emails/abc-123"
        );
        let long: Vec<&str> = std::iter::repeat_n("a", 9).collect();
        for bad in [vec![], vec![""], vec!["."], vec![".."], long] {
            assert!(url(&bad).is_err());
        }
    }

    #[test]
    fn control_bytes_are_refused_before_any_request_is_built() {
        for value in ["a\nb", "a\rb", "a\0b", "a\u{7f}b"] {
            assert_eq!(url(&[value]), Err("lnemail_invalid_input"));
            assert_eq!(authorization(value), Err("lnemail_invalid_input"));
        }
    }

    #[test]
    fn an_empty_token_carries_no_authorization_header() {
        assert!(authorization("").unwrap().is_none());
        assert!(authorization("secret").unwrap().is_some());
    }

    #[tokio::test]
    async fn oversized_response_is_refused_before_body_collection() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                        MAX_RESPONSE + 1
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            execute(
                &client,
                reqwest::Method::GET,
                format!("http://{address}"),
                None,
                None,
            )
            .await,
            Err("lnemail_response_too_large")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_server_that_never_answers_is_given_up_on() {
        use tokio::{io::AsyncReadExt, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            let ended = tokio::time::timeout(Duration::from_secs(1), socket.read(&mut request))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(ended, 0, "the request must be dropped, not left open");
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let outcome = tokio::time::timeout(
            Duration::from_millis(200),
            execute(
                &client,
                reqwest::Method::GET,
                format!("http://{address}"),
                None,
                None,
            ),
        )
        .await;
        assert!(outcome.is_err() || outcome.unwrap().is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_unauthorized_status_is_reported_before_the_body_is_read() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            execute(
                &client,
                reqwest::Method::GET,
                format!("http://{address}"),
                None,
                None,
            )
            .await,
            Err("lnemail_unauthorized")
        );
        server.await.unwrap();
    }
}
