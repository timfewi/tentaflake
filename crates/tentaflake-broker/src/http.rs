use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json".into(),
            body: serde_json::to_vec(&value)
                .unwrap_or_else(|_| b"{\"error\":\"encoding\"}".to_vec()),
        }
    }
}

pub fn read_request(
    stream: &mut TcpStream,
    max_header_bytes: usize,
    max_body_bytes: usize,
) -> Result<Request, String> {
    let mut raw = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = stream
            .read(&mut chunk)
            .map_err(|error| format!("read failed: {error}"))?;
        if count == 0 {
            return Err("connection closed before request headers".into());
        }
        raw.extend_from_slice(&chunk[..count]);
        if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if raw.len() > max_header_bytes {
            return Err("request headers exceed configured limit".into());
        }
    };
    if header_end > max_header_bytes {
        return Err("request headers exceed configured limit".into());
    }

    let header_text =
        std::str::from_utf8(&raw[..header_end - 4]).map_err(|_| "request headers are not UTF-8")?;
    if header_text.chars().any(|character| {
        character.is_control() && character != '\r' && character != '\n' && character != '\t'
    }) {
        return Err("request headers contain control characters".into());
    }
    let mut lines = header_text.split("\r\n");
    let start = lines.next().ok_or("missing request line")?;
    let fields: Vec<_> = start.split(' ').collect();
    if fields.len() != 3 || fields[2] != "HTTP/1.1" {
        return Err("only strict HTTP/1.1 requests are accepted".into());
    }
    if fields[1].is_empty() || !fields[1].starts_with('/') || fields[1].contains('#') {
        return Err("invalid request target".into());
    }
    let method = fields[0].to_string();
    let path = fields[1].to_string();

    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or("malformed request header")?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return Err("invalid request header name".into());
        }
        let name = name.to_ascii_lowercase();
        if headers.insert(name, value.trim().to_string()).is_some() {
            return Err("duplicate request headers are rejected".into());
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err("transfer encoding is not accepted".into());
    }
    let content_length = match headers.get("content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "invalid content-length")?,
        None => 0,
    };
    if content_length > max_body_bytes {
        return Err("request body exceeds configured limit".into());
    }
    let buffered = raw.len() - header_end;
    if buffered > content_length {
        return Err("pipelined requests are not accepted".into());
    }
    while raw.len() - header_end < content_length {
        let remaining = content_length - (raw.len() - header_end);
        let read_length = remaining.min(chunk.len());
        let count = stream
            .read(&mut chunk[..read_length])
            .map_err(|error| format!("body read failed: {error}"))?;
        if count == 0 {
            return Err("connection closed before request body".into());
        }
        raw.extend_from_slice(&chunk[..count]);
    }
    Ok(Request {
        method,
        path,
        headers,
        body: raw[header_end..].to_vec(),
    })
}

pub fn write_response(stream: &mut TcpStream, response: Response) -> std::io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    )?;
    stream.write_all(&response.body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn parse(raw: &'static [u8]) -> Result<Request, String> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let writer = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(raw).unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();
        let result = read_request(&mut stream, 1024, 1024);
        writer.join().unwrap();
        result
    }

    #[test]
    fn parses_bounded_request() {
        let request =
            parse(b"POST /v1/fetch HTTP/1.1\r\nHost: local\r\nContent-Length: 2\r\n\r\n{}")
                .unwrap();
        assert_eq!(request.body, b"{}");
    }

    #[test]
    fn rejects_duplicate_and_chunked_headers() {
        assert!(parse(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n").is_err());
        assert!(parse(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").is_err());
    }
}
