//! A bounded readiness exchange on a caller-authenticated, held relay stream.
//! The caller brackets the relay/service/controller identities and fresh lease;
//! this exchange neither authenticates those processes nor checks providers.
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_RESPONSE: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(2);
const UNAVAILABLE: &str = "Research readiness unavailable";
const INVALID: &str = "invalid Research readiness response";
const HELLO: &[u8] = br#"{"version":1,"id":1,"operation":{"kind":"hello"}}"#;
const READINESS: &[u8] = br#"{"version":1,"id":2,"operation":{"kind":"call","tool":"research_job","arguments":{"operation":"readiness"}}}"#;
const TOOLS: [&str; 5] = [
    "research_job",
    "research_search",
    "research_fetch",
    "research_browser",
    "research_read",
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReadyReport {
    pub policy_sha256: String,
    pub valid_until: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response<T> {
    version: u32,
    id: u64,
    outcome: Outcome<T>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Outcome<T> {
    Result { value: T },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    version: u32,
    max_frame: u32,
    tools: [String; 5],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Readiness {
    schema_version: u32,
    caller_uid: u32,
    policy_sha256: String,
    accepting: bool,
    observed_at: u64,
    egress: Egress,
    untrusted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Egress {
    mode: String,
    generation: String,
    valid_until: u64,
    lease_valid: bool,
}

fn generation(value: &str) -> bool {
    value.len() == 36
        && value != "00000000-0000-0000-0000-000000000000"
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| UNAVAILABLE.into())
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| UNAVAILABLE.into())
}

fn send(stream: &mut UnixStream, body: &[u8], deadline: Instant) -> Result<(), String> {
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);
    let mut bytes = frame.as_slice();
    while !bytes.is_empty() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(|_| UNAVAILABLE)?;
        match stream.write(bytes) {
            Ok(0) => return Err(UNAVAILABLE.into()),
            Ok(length) => bytes = &bytes[length..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(UNAVAILABLE.into()),
        }
    }
    remaining(deadline)?;
    Ok(())
}

fn read_exact(
    stream: &mut UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), String> {
    // Recompute the remaining total time on every partial read. read_exact with
    // one per-read socket timeout alone would permit a slow partial-frame peer.
    while !bytes.is_empty() {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|_| UNAVAILABLE)?;
        match stream.read(bytes) {
            Ok(0) => return Err(UNAVAILABLE.into()),
            Ok(length) => bytes = &mut bytes[length..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(UNAVAILABLE.into()),
        }
    }
    remaining(deadline)?;
    Ok(())
}

fn decode<T: DeserializeOwned>(bytes: &[u8], id: u64) -> Result<T, String> {
    if bytes.is_empty() || bytes.len() > MAX_RESPONSE {
        return Err(INVALID.into());
    }
    let response: Response<T> = serde_json::from_slice(bytes).map_err(|_| INVALID)?;
    if response.version != 1 || response.id != id {
        return Err(INVALID.into());
    }
    let Outcome::Result { value } = response.outcome;
    Ok(value)
}

fn receive<T: DeserializeOwned>(
    stream: &mut UnixStream,
    id: u64,
    deadline: Instant,
) -> Result<T, String> {
    let mut prefix = [0; 4];
    read_exact(stream, &mut prefix, deadline)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if !(1..=MAX_RESPONSE).contains(&length) {
        return Err(INVALID.into());
    }
    let mut bytes = vec![0; length];
    read_exact(stream, &mut bytes, deadline)?;
    decode(&bytes, id)
}

fn validate_report(
    report: Readiness,
    relay_uid: u32,
    expected_generation: &str,
    started: u64,
    finished: u64,
) -> Result<ReadyReport, String> {
    if report.schema_version != 1
        || report.caller_uid != relay_uid
        || !report.accepting
        || !report.untrusted
        || started > finished
        || !(started..=finished).contains(&report.observed_at)
        || report.egress.mode != "ready"
        || !report.egress.lease_valid
        || report.egress.generation != expected_generation
        || report.egress.valid_until <= finished
        || report.egress.valid_until > report.observed_at.saturating_add(10)
        || report.policy_sha256.len() != 64
        || !report
            .policy_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(INVALID.into());
    }
    Ok(ReadyReport {
        policy_sha256: report.policy_sha256,
        valid_until: report.egress.valid_until,
    })
}

/// Use one fresh blocking stream, with no retry or fallback after either send.
/// Keep it open after success for the caller's exact peer/process re-observation.
pub(crate) fn probe(
    stream: &mut UnixStream,
    relay_uid: u32,
    expected_generation: &str,
    deadline: Instant,
) -> Result<ReadyReport, String> {
    if relay_uid == 0 || relay_uid == u32::MAX || !generation(expected_generation) {
        return Err("invalid Research readiness request context".into());
    }
    let deadline = deadline.min(Instant::now() + TIMEOUT);
    let started = now()?;
    send(stream, HELLO, deadline)?;
    let hello: Hello = receive(stream, 1, deadline)?;
    if hello.version != 1 || hello.max_frame != 1024 * 1024 || hello.tools != TOOLS {
        return Err(INVALID.into());
    }
    send(stream, READINESS, deadline)?;
    let report = receive(stream, 2, deadline)?;
    let report = validate_report(report, relay_uid, expected_generation, started, now()?)?;
    remaining(deadline)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::thread;

    const GENERATION: &str = "11111111-2222-4333-8444-555555555555";
    const UID: u32 = 62107;

    fn value(observed_at: u64) -> Value {
        json!({
            "schema_version":1, "caller_uid":UID, "policy_sha256":"a".repeat(64),
            "accepting":true, "observed_at":observed_at, "untrusted":true,
            "egress":{"mode":"ready", "generation":GENERATION,
                "valid_until":observed_at + 5, "lease_valid":true}
        })
    }

    fn response(id: u64, value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"version":1,"id":id,
            "outcome":{"kind":"result","value":value}}))
        .unwrap()
    }

    fn hello() -> Vec<u8> {
        response(1, json!({"version":1,"max_frame":1048576,"tools":TOOLS}))
    }

    #[test]
    fn readiness_requires_exact_authority_state_and_fresh_observation() {
        let base = value(100);
        let parse =
            |value| validate_report(decode(&response(2, value), 2)?, UID, GENERATION, 99, 101);
        assert_eq!(parse(base.clone()).unwrap().valid_until, 105);
        for (field, value) in [
            ("schema_version", json!(2)),
            ("caller_uid", json!(UID + 1)),
            ("accepting", json!(false)),
            ("untrusted", json!(false)),
            ("observed_at", json!(98)),
            ("observed_at", json!(102)),
            ("policy_sha256", json!("A".repeat(64))),
            ("policy_sha256", json!("a".repeat(63))),
            ("extra", json!(true)),
        ] {
            let mut changed = base.clone();
            changed[field] = value;
            assert!(parse(changed).is_err(), "field {field}");
        }
        for (field, value) in [
            ("mode", json!("offline")),
            ("mode", json!("draining")),
            ("lease_valid", json!(false)),
            ("generation", json!(null)),
            ("generation", json!("00000000-0000-0000-0000-000000000000")),
            ("generation", json!("21111111-2222-4333-8444-555555555555")),
            ("valid_until", json!(101)),
            ("valid_until", json!(111)),
            ("extra", json!(true)),
        ] {
            let mut changed = base.clone();
            changed["egress"][field] = value;
            assert!(parse(changed).is_err(), "egress field {field}");
        }
        assert!(
            validate_report(
                decode(&response(2, base), 2).unwrap(),
                UID,
                GENERATION,
                102,
                101
            )
            .is_err()
        );
    }

    #[test]
    fn closed_envelopes_refuse_duplicate_fields_and_do_not_echo_peer_errors() {
        let raw = String::from_utf8(response(2, value(100))).unwrap();
        for changed in [
            raw.replacen("\"version\":1", "\"version\":2", 1),
            raw.replacen("\"id\":2", "\"id\":1", 1),
            raw.replacen("\"id\":2", "\"id\":2,\"id\":2", 1),
            raw.replacen(
                "\"accepting\":true",
                "\"accepting\":true,\"accepting\":true",
                1,
            ),
            raw.replacen(
                "\"mode\":\"ready\"",
                "\"mode\":\"ready\",\"mode\":\"ready\"",
                1,
            ),
            raw.replacen("\"kind\":\"result\"", "\"kind\":\"progress\"", 1),
            raw.replacen("\"version\":1", "\"version\":1,\"extra\":true", 1),
            r#"{"version":1,"id":2,"outcome":{"kind":"error","code":"PRIVATE PEER DATA"}}"#.into(),
        ] {
            assert_eq!(
                decode::<Readiness>(changed.as_bytes(), 2).err().unwrap(),
                INVALID
            );
        }
        assert!(decode::<Readiness>(&vec![b' '; MAX_RESPONSE + 1], 2).is_err());
    }

    fn read_request(stream: &mut UnixStream, expected: &[u8]) {
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).unwrap();
        assert_eq!(u32::from_be_bytes(prefix) as usize, expected.len());
        let mut bytes = vec![0; expected.len()];
        stream.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn real_stream_negotiates_then_probes_without_jobs_or_retries() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let serving = thread::spawn(move || {
            read_request(&mut server, HELLO);
            send(&mut server, &hello(), Instant::now() + TIMEOUT).unwrap();
            read_request(&mut server, READINESS);
            let body = response(2, value(now().unwrap()));
            let mut frame = (body.len() as u32).to_be_bytes().to_vec();
            frame.extend(body);
            server.set_write_timeout(Some(TIMEOUT)).unwrap();
            for part in frame.chunks(3) {
                server.write_all(part).unwrap();
            }
            let mut extra = [0];
            assert_eq!(server.read(&mut extra).unwrap(), 0);
        });
        let result = probe(&mut client, UID, GENERATION, Instant::now() + TIMEOUT);
        drop(client);
        serving.join().unwrap();
        assert_eq!(result.unwrap().policy_sha256, "a".repeat(64));
    }

    #[test]
    fn real_stream_refuses_oversized_or_old_protocol_before_readiness_dispatch() {
        for bad in [
            None,
            Some(response(
                1,
                json!({"version":2,"max_frame":1048576,"tools":TOOLS}),
            )),
        ] {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            let serving = thread::spawn(move || {
                read_request(&mut server, HELLO);
                if let Some(body) = bad {
                    send(&mut server, &body, Instant::now() + TIMEOUT).unwrap();
                } else {
                    server
                        .write_all(&((MAX_RESPONSE + 1) as u32).to_be_bytes())
                        .unwrap();
                }
                let mut extra = [0];
                assert_eq!(server.read(&mut extra).unwrap(), 0);
            });
            let result = probe(&mut client, UID, GENERATION, Instant::now() + TIMEOUT);
            drop(client);
            serving.join().unwrap();
            assert!(result.is_err());
        }
    }

    #[test]
    fn real_partial_frames_do_not_extend_the_external_deadline() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let serving = thread::spawn(move || {
            read_request(&mut server, HELLO);
            let body = hello();
            let mut frame = (body.len() as u32).to_be_bytes().to_vec();
            frame.extend(body);
            for byte in frame {
                if server.write_all(&[byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        let started = Instant::now();
        let result = probe(
            &mut client,
            UID,
            GENERATION,
            started + Duration::from_millis(80),
        );
        let elapsed = started.elapsed();
        drop(client);
        serving.join().unwrap();
        assert!(result.is_err());
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn invalid_context_or_elapsed_deadline_never_sends_hello() {
        for (uid, generation, deadline) in [
            (0, GENERATION, Instant::now() + TIMEOUT),
            (u32::MAX, GENERATION, Instant::now() + TIMEOUT),
            (
                UID,
                "00000000-0000-0000-0000-000000000000",
                Instant::now() + TIMEOUT,
            ),
            (
                UID,
                "AAAAAAAA-2222-4333-8444-555555555555",
                Instant::now() + TIMEOUT,
            ),
            (UID, GENERATION, Instant::now()),
        ] {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            assert!(probe(&mut client, uid, generation, deadline).is_err());
            drop(client);
            let mut byte = [0];
            assert_eq!(server.read(&mut byte).unwrap(), 0);
        }
    }
}
