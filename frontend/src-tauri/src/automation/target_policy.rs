use std::net::{IpAddr, SocketAddr};
use url::Url;

#[derive(Debug, Clone)]
pub struct PinnedTarget {
    pub url: Url,
    pub host: String,
    pub addresses: Vec<SocketAddr>,
}

pub async fn resolve_and_validate(url: &str, approved: &[String]) -> Result<PinnedTarget, String> {
    let url = Url::parse(url).map_err(|_| "Webhook URL is invalid".to_owned())?;
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err("Webhook URL must be HTTPS without userinfo, query, or fragment".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "Webhook URL must contain a host".to_owned())?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !approved
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(&host))
    {
        return Err("Webhook host is not explicitly allowlisted".into());
    }
    let mut url = url;
    url.set_host(Some(&host))
        .map_err(|_| "Webhook host is invalid".to_owned())?;
    let port = url.port_or_known_default().unwrap_or(443);
    let resolved = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| "Webhook host could not be resolved".to_owned())?;
    let mut addresses = resolved.collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err("Webhook host resolved to no addresses".into());
    }
    validate_addresses(&host, &addresses, approved)?;
    Ok(PinnedTarget {
        url,
        host,
        addresses,
    })
}

fn validate_addresses(
    host: &str,
    addresses: &[SocketAddr],
    approved: &[String],
) -> Result<(), String> {
    if !approved
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(host))
    {
        return Err("Webhook host is not explicitly allowlisted".into());
    }
    let approved_ips: Vec<IpAddr> = approved
        .iter()
        .filter_map(|entry| entry.parse::<IpAddr>().ok())
        .collect();
    for address in addresses {
        if !is_public_ip(address.ip()) && !approved_ips.contains(&address.ip()) {
            return Err(
                "Webhook host resolved to a private, link-local, or reserved address".into(),
            );
        }
        if !approved_ips.is_empty() && !approved_ips.contains(&address.ip()) {
            return Err("Webhook DNS answer changed after target approval".into());
        }
    }
    Ok(())
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a >= 224
                || (a == 255 && b == 255 && c == 255 && d == 255))
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            (segments[0] & 0xe000) == 0x2000 && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_public_ip, resolve_and_validate};
    use std::net::SocketAddr;

    #[test]
    fn blocks_private_metadata_link_local_and_reserved_addresses() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "100.64.0.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "2001:db8::1",
        ] {
            assert!(!is_public_ip(address.parse().unwrap()), "{address}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[tokio::test]
    async fn rejects_http_unapproved_and_dns_rebinding_targets() {
        assert!(
            resolve_and_validate("http://example.com/hook", &["example.com".into()])
                .await
                .is_err()
        );
        assert!(
            resolve_and_validate("https://example.com/hook", &["other.example".into()])
                .await
                .is_err()
        );
        assert!(
            resolve_and_validate("https://localhost/hook", &["localhost".into()])
                .await
                .is_err()
        );
        let rebinding = SocketAddr::from(([169, 254, 169, 254], 443));
        let approved = ["example.com".to_owned(), "93.184.216.34".to_owned()];
        assert!(super::validate_addresses("example.com", &[rebinding], &approved).is_err());
    }

    #[tokio::test]
    async fn allows_private_ip_only_when_that_exact_ip_is_approved() {
        let target = resolve_and_validate("https://127.0.0.1/hook", &["127.0.0.1".into()])
            .await
            .unwrap();
        assert_eq!(target.host, "127.0.0.1");
        assert!(target
            .addresses
            .iter()
            .all(|address| address.ip().is_loopback()));
    }
}
