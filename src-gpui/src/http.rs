use std::io::Read;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use reqwest::{StatusCode, Url, blocking::Client};

const BODY_LIMIT: usize = 2_000_000;
const PRETTY_JSON_LIMIT: usize = 256 * 1024;
static CLIENT: OnceLock<Result<Client, reqwest::Error>> = OnceLock::new();

pub struct Response {
    pub info: ResponseInfo,
    pub body: String,
}

pub struct ResponseInfo {
    pub status: StatusCode,
    pub details: String,
}

pub fn get(input: &str) -> Result<Response, String> {
    let url = Url::parse(input)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
        .ok_or_else(|| "Enter a valid HTTP or HTTPS URL.".to_owned())?;

    // Initialize TLS and the connection pool off the UI thread, on the first send.
    let client = CLIENT
        .get_or_init(|| {
            Client::builder()
                .user_agent("Postlite")
                .timeout(Duration::from_secs(60))
                .build()
        })
        .as_ref()
        .map_err(|error| error.to_string())?;

    let started = Instant::now();
    let response = client
        .get(url)
        .send()
        .map_err(|error| error.without_url().to_string())?;
    let status = response.status();
    let mut data = Vec::new();
    response
        .take((BODY_LIMIT + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    let elapsed_ms = started.elapsed().as_millis();

    let truncated = data.len() > BODY_LIMIT;
    data.truncate(BODY_LIMIT);
    let details = if truncated {
        format!("{elapsed_ms} ms · {BODY_LIMIT} B preview (truncated)")
    } else {
        format!("{elapsed_ms} ms · {} B", data.len())
    };
    let body = if !truncated && data.len() <= PRETTY_JSON_LIMIT {
        serde_json::from_slice::<serde_json::Value>(&data)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .filter(|pretty| pretty.len() <= BODY_LIMIT)
    } else {
        None
    }
    .unwrap_or_else(|| String::from_utf8_lossy(&data).into_owned());

    Ok(Response {
        info: ResponseInfo { status, details },
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn bounds_large_response_bodies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&mut stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            let size = BODY_LIMIT + 1;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.write_all(&vec![b'x'; size]).unwrap();
        });
        let response = get(&url).unwrap();
        server.join().unwrap();

        assert_eq!(response.info.status, StatusCode::OK);
        assert_eq!(response.body.len(), BODY_LIMIT);
        assert!(response.info.details.contains("truncated"));
    }
}
