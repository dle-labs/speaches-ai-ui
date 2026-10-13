use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use reqwest::{Client, RequestBuilder, Response, Url, multipart, redirect::Policy};
use serde::Deserialize;

const BODY_LIMIT: usize = 2 * 1024 * 1024;

fn endpoint(base: &str, resource: &str) -> Result<Url> {
    let base = base.trim();
    // Reject syntax that URL parsers can silently normalize, including empty userinfo.
    if base.contains('\\') || base.chars().any(char::is_control) {
        bail!("Server address must be an absolute HTTP(S) URL");
    }
    let mut url =
        Url::parse(base).map_err(|_| anyhow!("Server address must be an absolute HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("Server address must be an absolute HTTP(S) URL");
    }
    let authority = base
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .ok_or_else(|| anyhow!("Server address must be an absolute HTTP(S) URL"))?;
    if authority.is_empty() {
        bail!("Server address must be an absolute HTTP(S) URL");
    }
    if authority.contains('@') || !url.username().is_empty() || url.password().is_some() {
        bail!("Server address must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("Server address must not contain a query or fragment");
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/v1") {
        format!("{path}/{resource}")
    } else {
        format!("{path}/v1/{resource}")
    };
    url.set_path(&path);
    Ok(url)
}

fn client() -> Result<Client> {
    Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| anyhow!("Could not initialize the HTTP client"))
}

fn authorize(request: RequestBuilder, api_key: &str) -> RequestBuilder {
    if api_key.trim().is_empty() {
        request
    } else {
        request.bearer_auth(api_key.trim())
    }
}

async fn response_body(request: RequestBuilder, operation: &str) -> Result<(Vec<u8>, u16)> {
    // Deliberately discard underlying errors: their URLs or payloads may contain secrets.
    let mut response: Response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            anyhow!("{operation} request timed out")
        } else {
            anyhow!("{operation} request failed; check the server address and connection")
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        bail!("{operation} request failed (HTTP {})", status.as_u16());
    }
    if response
        .content_length()
        .is_some_and(|size| size > BODY_LIMIT as u64)
    {
        bail!(
            "{operation} response exceeded 2 MiB (HTTP {})",
            status.as_u16()
        );
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        anyhow!(
            "Could not read {operation} response (HTTP {})",
            status.as_u16()
        )
    })? {
        if chunk.len() > BODY_LIMIT - body.len() {
            bail!(
                "{operation} response exceeded 2 MiB (HTTP {})",
                status.as_u16()
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, status.as_u16()))
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    task: Option<String>,
}

pub(crate) async fn models(base: &str, api_key: &str) -> Result<Vec<String>> {
    let url = endpoint(base, "models")?;
    let request = authorize(client()?.get(url), api_key).timeout(Duration::from_secs(10));
    let (body, status) = response_body(request, "Model list").await?;
    let response: ModelList = serde_json::from_slice(&body)
        .map_err(|_| anyhow!("Invalid model list response (HTTP {status})"))?;
    Ok(response
        .data
        .into_iter()
        .filter(|model| {
            model
                .task
                .as_deref()
                .is_none_or(|task| task == "automatic-speech-recognition")
                && !model.id.trim().is_empty()
        })
        .map(|model| model.id)
        .collect())
}

#[derive(Deserialize)]
struct Transcription {
    text: String,
}

pub(crate) async fn transcribe(
    base: &str,
    api_key: &str,
    model: &str,
    language: &str,
    wav: Vec<u8>,
) -> Result<String> {
    let url = endpoint(base, "audio/transcriptions")?;
    let file = multipart::Part::bytes(wav)
        .file_name("recording.wav")
        .mime_str("audio/wav")
        .map_err(|_| anyhow!("Could not prepare the audio upload"))?;
    let mut form = multipart::Form::new()
        .text("model", model.to_owned())
        .part("file", file)
        .text("response_format", "json")
        .text("stream", "false");
    if !language.trim().is_empty() {
        form = form.text("language", language.trim().to_owned());
    }
    let request = authorize(client()?.post(url), api_key)
        .timeout(Duration::from_secs(180))
        .multipart(form);
    let (body, status) = response_body(request, "Transcription").await?;
    let response: Transcription = serde_json::from_slice(&body)
        .map_err(|_| anyhow!("Invalid transcription response (HTTP {status})"))?;
    Ok(response.text)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread::{self, JoinHandle},
        time::Instant,
    };

    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn mock(response: String) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "No mock request received");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("Mock accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "Incomplete mock request");
                request.extend_from_slice(&buffer[..count]);
                assert!(request.len() < BODY_LIMIT);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            // Oversize-body tests intentionally let the client close before all bytes arrive.
            let _ = stream.write_all(response.as_bytes());
            String::from_utf8(request).unwrap()
        });
        (address, handle)
    }

    fn json_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn endpoint_preserves_prefix_and_normalizes_version_and_slashes() {
        for (base, expected) in [
            ("http://localhost:9791", "http://localhost:9791/v1/models"),
            ("http://localhost:9791/", "http://localhost:9791/v1/models"),
            (
                "http://localhost:9791/v1/",
                "http://localhost:9791/v1/models",
            ),
            (
                "https://example.test/proxy",
                "https://example.test/proxy/v1/models",
            ),
            (
                "https://example.test/proxy/v1///",
                "https://example.test/proxy/v1/models",
            ),
            (
                "https://example.test/a%20b/v1",
                "https://example.test/a%20b/v1/models",
            ),
        ] {
            assert_eq!(endpoint(base, "models").unwrap().as_str(), expected);
        }
    }

    #[test]
    fn endpoint_rejects_credentials_queries_fragments_and_non_http_urls() {
        for base in [
            "http://user:secret@example.test",
            "http://user@example.test",
            "http://@example.test",
            "http://example.test?token=secret",
            "http://example.test?",
            "http://example.test#secret",
            "http://example.test#",
            "ftp://example.test",
            "file:///etc/passwd",
            "localhost:9791",
            "http:\\example.test",
            "http://exam\nple.test",
            "http:///example.test",
        ] {
            assert!(endpoint(base, "models").is_err(), "Accepted {base}");
        }
    }

    #[test]
    fn models_filters_tasks_and_works_without_authentication() {
        let (base, server) = mock(json_response(
            r#"{"data":[{"id":"deepdml/faster-whisper-large-v3-turbo-ct2","task":"automatic-speech-recognition"},{"id":"Systran/faster-whisper-small.en"},{"id":"voice","task":"text-to-speech"}]}"#,
        ));
        let result = runtime()
            .block_on(models(&format!("{base}/proxy/v1/"), ""))
            .unwrap();
        assert_eq!(
            result,
            [
                "deepdml/faster-whisper-large-v3-turbo-ct2",
                "Systran/faster-whisper-small.en"
            ]
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /proxy/v1/models HTTP/1.1\r\n"));
        assert!(!request.to_lowercase().contains("authorization:"));
    }

    #[test]
    fn transcription_posts_wav_and_required_multipart_fields() {
        let (base, server) = mock(json_response(r#"{"text":"Hello, world!"}"#));
        let result = runtime()
            .block_on(transcribe(
                &base,
                "test-key",
                "whisper",
                " en ",
                b"RIFF-WAVE-fixture".to_vec(),
            ))
            .unwrap();
        assert_eq!(result, "Hello, world!");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"));
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer test-key\r\n")
        );
        assert!(
            request
                .to_lowercase()
                .contains("content-type: audio/wav\r\n")
        );
        for field in [
            "name=\"model\"\r\n\r\nwhisper",
            "name=\"response_format\"\r\n\r\njson",
            "name=\"stream\"\r\n\r\nfalse",
            "name=\"language\"\r\n\r\nen",
            "name=\"file\"; filename=\"recording.wav\"",
            "RIFF-WAVE-fixture",
        ] {
            assert!(request.contains(field), "Missing multipart field: {field}");
        }
    }

    #[test]
    fn transcription_omits_blank_language() {
        let (base, server) = mock(json_response(r#"{"text":""}"#));
        runtime()
            .block_on(transcribe(&base, "", "whisper", " \t ", vec![]))
            .unwrap();
        assert!(!server.join().unwrap().contains("name=\"language\""));
    }

    #[test]
    fn redirects_do_not_forward_audio_or_credentials() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let response = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            target.local_addr().unwrap()
        );
        let (base, server) = mock(response);
        let error = runtime()
            .block_on(transcribe(
                &base,
                "secret",
                "whisper",
                "",
                b"audio".to_vec(),
            ))
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.to_string(), "Transcription request failed (HTTP 307)");
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn errors_do_not_expose_server_response_or_secrets() {
        let (base, server) = mock("HTTP/1.1 401 Unauthorized\r\nContent-Length: 20\r\nConnection: close\r\n\r\nprivate-secret-audio".to_owned());
        let error = runtime()
            .block_on(models(&base, "private-api-key"))
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(format!("{error:#}"), "Model list request failed (HTTP 401)");
    }

    #[test]
    fn invalid_transcription_json_is_not_echoed() {
        let (base, server) = mock(json_response(r#"{"text":{"private-transcript":"secret"}}"#));
        let error = runtime()
            .block_on(transcribe(&base, "", "whisper", "", vec![]))
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(
            format!("{error:#}"),
            "Invalid transcription response (HTTP 200)"
        );
    }

    #[test]
    fn chunked_response_cannot_exceed_body_limit() {
        let body = "x".repeat(BODY_LIMIT + 1);
        let response = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n{body}\r\n0\r\n\r\n",
            body.len()
        );
        let (base, server) = mock(response);
        let error = runtime().block_on(models(&base, "")).unwrap_err();
        server.join().unwrap();
        assert_eq!(
            error.to_string(),
            "Model list response exceeded 2 MiB (HTTP 200)"
        );
    }
}
