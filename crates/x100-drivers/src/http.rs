//! Just enough HTTP/1.1 for the fake provider and for harness hooks: one request per connection,
//! answered with `Connection: close`, which both harnesses' clients accept.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_HEAD: usize = 1 << 20;

pub(crate) struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub(crate) async fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(end) = find(&buf, b"\r\n\r\n") {
            break end;
        }
        if buf.len() > MAX_HEAD {
            return Err(std::io::Error::other("request head too large"));
        }
        let mut chunk = [0u8; 8192];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_owned();
    let target = first.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let mut request = Request {
        method,
        target,
        headers,
        body: buf[head_end + 4..].to_vec(),
    };
    if request
        .header("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        request.body = read_chunked(stream, std::mem::take(&mut request.body)).await?;
    } else {
        let length: usize = request
            .header("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        while request.body.len() < length {
            let mut chunk = vec![0u8; length - request.body.len()];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            request.body.extend_from_slice(&chunk[..n]);
        }
    }
    Ok(request)
}

async fn read_chunked(stream: &mut TcpStream, mut raw: Vec<u8>) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        while find(&raw, b"\r\n").is_none() {
            fill(stream, &mut raw).await?;
        }
        let line_end = find(&raw, b"\r\n").expect("found");
        let size_text = String::from_utf8_lossy(&raw[..line_end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("0").trim(), 16)
            .map_err(|_| std::io::Error::other("bad chunk size"))?;
        while raw.len() < line_end + 2 + size + 2 {
            fill(stream, &mut raw).await?;
        }
        if size == 0 {
            return Ok(body);
        }
        body.extend_from_slice(&raw[line_end + 2..line_end + 2 + size]);
        raw.drain(..line_end + 2 + size + 2);
    }
}

async fn fill(stream: &mut TcpStream, raw: &mut Vec<u8>) -> std::io::Result<()> {
    let mut chunk = [0u8; 8192];
    let n = stream.read(&mut chunk).await?;
    if n == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    raw.extend_from_slice(&chunk[..n]);
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub(crate) async fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    stream.shutdown().await
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}
