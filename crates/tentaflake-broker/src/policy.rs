use reqwest::Url;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

pub struct ResolvedTarget {
    pub url: Url,
    pub host: String,
    pub addresses: Vec<SocketAddr>,
}

pub fn resolve_public_target(
    raw_url: &str,
    allowed_hosts: &[String],
    allow_http: bool,
    allow_non_public_for_tests: bool,
) -> Result<ResolvedTarget, String> {
    resolve_public_target_with(
        raw_url,
        allowed_hosts,
        allow_http,
        allow_non_public_for_tests,
        |host, port| {
            (host, port)
                .to_socket_addrs()
                .map(|addresses| addresses.collect())
                .map_err(|_| "target DNS resolution failed".to_string())
        },
    )
}

fn resolve_public_target_with<F>(
    raw_url: &str,
    allowed_hosts: &[String],
    allow_http: bool,
    allow_non_public_for_tests: bool,
    resolver: F,
) -> Result<ResolvedTarget, String>
where
    F: FnOnce(&str, u16) -> Result<Vec<SocketAddr>, String>,
{
    let url = Url::parse(raw_url).map_err(|_| "target is not a valid URL")?;
    if url.scheme() != "https" && !(allow_http && url.scheme() == "http") {
        return Err("target scheme is not allowed".into());
    }
    if url.username() != "" || url.password().is_some() || url.fragment().is_some() {
        return Err("target URL credentials and fragments are rejected".into());
    }
    let host = url
        .host_str()
        .ok_or("target URL has no host")?
        .to_ascii_lowercase();
    if !allowed_hosts.iter().any(|allowed| allowed == &host) {
        return Err("target host is not in the exact allowlist".into());
    }
    let port = url
        .port_or_known_default()
        .ok_or("target has no known port")?;
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        resolver(host.as_str(), port)?
    };
    if addresses.is_empty()
        || (!allow_non_public_for_tests
            && addresses.iter().any(|address| !is_public_ip(address.ip())))
    {
        return Err("target resolves to a blocked or non-public address".into());
    }
    addresses.iter().try_for_each(|address| {
        if address.port() == port {
            Ok(())
        } else {
            Err("resolved target port changed unexpectedly".to_string())
        }
    })?;
    Ok(ResolvedTarget {
        url,
        host,
        addresses,
    })
}

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_v4(mapped);
    }
    let segments = ip.segments();
    (segments[0] & 0xe000) == 0x2000 && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_metadata_private_tailnet_and_documentation_ranges() {
        for value in [
            "169.254.169.254",
            "10.0.0.1",
            "100.100.100.100",
            "127.0.0.1",
            "192.0.2.1",
            "fd7a:115c:a1e0::1",
            "2001:db8::1",
        ] {
            assert!(!is_public_ip(value.parse().unwrap()), "{value}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn requires_exact_host_and_safe_scheme_before_dns() {
        let hosts = vec!["example.com".into()];
        assert!(resolve_public_target("file:///etc/passwd", &hosts, false, false).is_err());
        assert!(resolve_public_target("https://sub.example.com/", &hosts, false, false).is_err());
        assert!(resolve_public_target("http://example.com/", &hosts, false, false).is_err());
    }

    #[test]
    fn blocks_ssrf_ip_literals_even_when_exactly_allowlisted() {
        for host in ["127.0.0.1", "169.254.169.254", "100.100.100.100"] {
            assert!(
                resolve_public_target(
                    &format!("https://{host}/latest/meta-data"),
                    &[host.into()],
                    false,
                    false,
                )
                .is_err(),
                "{host}"
            );
        }
    }

    #[test]
    fn revalidates_redirect_hosts_and_each_dns_answer() {
        let hosts = vec!["allowed.example".into()];
        let public = || {
            resolve_public_target_with(
                "https://allowed.example/start",
                &hosts,
                false,
                false,
                |_, port| Ok(vec![SocketAddr::new("1.1.1.1".parse().unwrap(), port)]),
            )
        };
        assert!(public().is_ok());

        let rebinding = resolve_public_target_with(
            "https://allowed.example/redirected",
            &hosts,
            false,
            false,
            |_, port| {
                Ok(vec![SocketAddr::new(
                    "169.254.169.254".parse().unwrap(),
                    port,
                )])
            },
        );
        assert!(rebinding.is_err());

        let redirect = resolve_public_target_with(
            "https://169.254.169.254/latest/meta-data",
            &hosts,
            false,
            false,
            |_, _| panic!("a denied redirect must not reach DNS"),
        );
        assert!(redirect.is_err());
    }
}
