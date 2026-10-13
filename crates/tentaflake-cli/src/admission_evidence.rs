use std::collections::BTreeSet;

const MAX_BROKER_HEALTH_BYTES: usize = 8192;

/// Validate the existing broker's bounded, non-streaming health response.
/// This proves only its reported local credential/audit readiness, not provider
/// authentication, available model budget, or the endpoint's instance identity.
pub(crate) fn broker_response_ready(response: &[u8]) -> bool {
    if response.len() > MAX_BROKER_HEALTH_BYTES {
        return false;
    }
    let Some(header_end) = response.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
        return false;
    };
    let Ok(headers) = std::str::from_utf8(&response[..header_end]) else {
        return false;
    };
    let mut lines = headers.split("\r\n");
    let mut status = lines.next().unwrap_or_default().splitn(3, ' ');
    if !matches!(status.next(), Some("HTTP/1.0" | "HTTP/1.1"))
        || status.next() != Some("200")
        || !status.next().is_some_and(valid_field_value)
    {
        return false;
    }

    let mut seen = BTreeSet::new();
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
            || !valid_field_value(value)
        {
            return false;
        }
        let name = name.to_ascii_lowercase();
        if !seen.insert(name.clone()) {
            return false;
        }
        let value = value.trim_matches([' ', '\t']);
        match name.as_str() {
            "content-length" => {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return false;
                }
                content_length = value.parse::<usize>().ok();
            }
            // The broker always supplies Content-Length. Ambiguous or encoded
            // framing must not turn a substring into positive health evidence.
            "transfer-encoding" => return false,
            "content-encoding" if !value.eq_ignore_ascii_case("identity") => return false,
            "content-type" if !value.eq_ignore_ascii_case("application/json") => return false,
            _ => {}
        }
    }
    let body = &response[header_end + 4..];
    if content_length != Some(body.len()) {
        return false;
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Health {
        status: String,
    }
    matches!(serde_json::from_slice::<Health>(body), Ok(health) if health.status == "ready")
}

fn valid_field_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte == b'\t' || (b' '..=b'~').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "{status}\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn exact_ready_health_accepts_broker_and_legacy_fixture_headers() {
        for version in ["HTTP/1.0", "HTTP/1.1"] {
            for headers in [
                "Connection: close\r\n",
                "Content-Type: application/json\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n",
            ] {
                assert!(broker_response_ready(&response(
                    &format!("{version} 200 OK"),
                    headers,
                    r#"{"status":"ready"}"#,
                )));
            }
        }
        assert!(broker_response_ready(&response(
            "HTTP/1.1 200 OK",
            "content-type: APPLICATION/JSON\r\n",
            " { \"status\" : \"ready\" } \n",
        )));
    }

    #[test]
    fn health_rejects_ready_substrings_outside_the_exact_json_contract() {
        for body in [
            r#"{"error":"unready"}"#,
            r#"{"details":{"status":"ready"}}"#,
            r#"{"status":"ready","error":"failed"}"#,
            r#"{"status":"ready","status":"unready"}"#,
            r#"{"status":"unready","status":"ready"}"#,
            r#"[{"status":"ready"}]"#,
            r#"{"status":"ready"} trailing"#,
            r#"{"status":"ready"}{"status":"ready"}"#,
            r#"{"status":true}"#,
            r#"{"status":"ready""#,
            "",
        ] {
            assert!(
                !broker_response_ready(&response("HTTP/1.1 200 OK", "", body)),
                "{body}"
            );
        }
        assert!(!broker_response_ready(&response(
            "HTTP/1.1 200 OK",
            "X-Diagnostic: {\"status\":\"ready\"}\r\n",
            r#"{"status":"unready"}"#,
        )));
    }

    #[test]
    fn health_rejects_wrong_status_and_ambiguous_http_framing() {
        for status in [
            "HTTP/1.1 503 Service Unavailable",
            "HTTP/1.1 2000 OK",
            "HTTP/2 200 OK",
            "HTTP/1.1  200 OK",
            "HTTP/1.1 200",
            "HTTP/1.1 200 OK\nInjected",
        ] {
            assert!(
                !broker_response_ready(&response(status, "", r#"{"status":"ready"}"#)),
                "{status}"
            );
        }
        for headers in [
            "Content-Length: 18\r\n",
            "content-length: 19\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Transfer-Encoding: identity\r\n",
            "Content-Encoding: gzip\r\n",
            "Content-Type: text/plain\r\n",
            "X-Probe: first\r\nx-probe: second\r\n",
            " Folded: header\r\n",
            "Invalid header\r\n",
            "X-Probe: value\nInjected\r\n",
        ] {
            assert!(
                !broker_response_ready(&response(
                    "HTTP/1.1 200 OK",
                    headers,
                    r#"{"status":"ready"}"#
                )),
                "{headers}"
            );
        }
        for length in [
            "",
            "17",
            "19",
            "+18",
            "18,18",
            "999999999999999999999999999999",
        ] {
            let malformed = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n{{\"status\":\"ready\"}}"
            );
            assert!(!broker_response_ready(malformed.as_bytes()), "{length}");
        }
        assert!(!broker_response_ready(
            b"HTTP/1.1 200 OK\r\n\r\n{\"status\":\"ready\"}"
        ));
    }

    #[test]
    fn health_rejects_oversized_truncated_and_non_utf8_responses() {
        let valid = response("HTTP/1.1 200 OK", "", r#"{"status":"ready"}"#);
        for length in 0..valid.len() {
            assert!(!broker_response_ready(&valid[..length]));
        }
        let padded = format!(
            "{}{{\"status\":\"ready\"}}",
            " ".repeat(MAX_BROKER_HEALTH_BYTES)
        );
        assert!(!broker_response_ready(&response(
            "HTTP/1.1 200 OK",
            "",
            &padded
        )));
        assert!(!broker_response_ready(
            b"HTTP/1.1 200 OK\r\nX-Probe: \xff\r\nContent-Length: 18\r\n\r\n{\"status\":\"ready\"}"
        ));
        assert!(!broker_response_ready(
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n\xff"
        ));
    }
}
