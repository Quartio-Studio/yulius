//! Host bindings: the destinations an agent secret may be sent to (egress proxy) or typed into
//! (browser fill).

use super::VaultError;

/// Hosts one secret may be bound to.
pub const MAX_HOSTS: usize = 16;

/// `api.example.com`, `*.example.com` (any subdomain, not the domain itself) or either with a
/// port (`api.example.com:8443`; 443 when absent).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostPattern {
    wildcard: bool,
    domain: String,
    port: u16,
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl HostPattern {
    /// Parses one binding (case-insensitive; an `https://` prefix and a trailing `/` are
    /// tolerated).
    ///
    /// # Errors
    /// [`VaultError::Host`] for anything but a DNS name with at least two labels, an optional
    /// leading `*.` and an optional port.
    pub fn parse(text: &str) -> Result<Self, VaultError> {
        let raw = text.trim();
        let lowered = raw.to_ascii_lowercase();
        let trimmed = lowered
            .strip_prefix("https://")
            .unwrap_or(&lowered)
            .trim_end_matches('/');
        let bad = || {
            VaultError::Host(format!(
                "invalid host {raw:?}: a host name like api.example.com or *.example.com, optionally with :port"
            ))
        };
        let (host, port) = match trimmed.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or_else(bad)?,
            ),
            None => (trimmed, 443),
        };
        let (wildcard, domain) = match host.strip_prefix("*.") {
            Some(domain) => (true, domain),
            None => (false, host),
        };
        let labels: Vec<&str> = domain.split('.').collect();
        let all_digits = labels.iter().all(|l| l.bytes().all(|b| b.is_ascii_digit()));
        if domain.len() > 253
            || labels.len() < 2
            || all_digits
            || !labels.iter().all(|l| valid_label(l))
        {
            return Err(bad());
        }
        Ok(Self {
            wildcard,
            domain: domain.to_owned(),
            port,
        })
    }

    /// Whether a request to `host:port` is covered (host compared case-insensitively, a
    /// trailing dot ignored).
    #[must_use]
    pub fn matches(&self, host: &str, port: u16) -> bool {
        if port != self.port {
            return false;
        }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if self.wildcard {
            host.len() > self.domain.len() + 1
                && host.ends_with(&self.domain)
                && host.as_bytes()[host.len() - self.domain.len() - 1] == b'.'
        } else {
            host == self.domain
        }
    }
}

impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.wildcard {
            f.write_str("*.")?;
        }
        f.write_str(&self.domain)?;
        if self.port != 443 {
            write!(f, ":{}", self.port)?;
        }
        Ok(())
    }
}

/// Parses a list of bindings (at most [`MAX_HOSTS`], duplicates removed). An empty list binds a
/// secret to nowhere: agents propose hosts and a person allows them (SPEC-121).
///
/// # Errors
/// [`VaultError::Host`] for a too long list or a bad entry.
pub fn parse_hosts(items: &[String]) -> Result<Vec<HostPattern>, VaultError> {
    let mut out: Vec<HostPattern> = Vec::new();
    for item in items.iter().filter(|i| !i.trim().is_empty()) {
        let pattern = HostPattern::parse(item)?;
        if !out.contains(&pattern) {
            out.push(pattern);
        }
    }
    if out.len() > MAX_HOSTS {
        return Err(VaultError::Host(format!("at most {MAX_HOSTS} hosts")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_wildcard_and_port_bindings() -> Result<(), VaultError> {
        let exact = HostPattern::parse("API.Stripe.com")?;
        assert!(exact.matches("api.stripe.com", 443));
        assert!(exact.matches("api.stripe.com.", 443));
        assert!(!exact.matches("api.stripe.com", 8443));
        assert!(!exact.matches("evil-api.stripe.com", 443));
        assert!(!exact.matches("api.stripe.com.evil.com", 443));
        assert_eq!(exact.to_string(), "api.stripe.com");

        let wild = HostPattern::parse("*.supabase.co")?;
        assert!(wild.matches("abc.supabase.co", 443));
        assert!(wild.matches("a.b.supabase.co", 443));
        assert!(!wild.matches("supabase.co", 443));
        assert!(!wild.matches("evilsupabase.co", 443));

        let port = HostPattern::parse("https://internal.example.com:8443/")?;
        assert!(port.matches("internal.example.com", 8443));
        assert_eq!(port.to_string(), "internal.example.com:8443");
        Ok(())
    }

    #[test]
    fn refuses_what_is_not_a_host_name() {
        for bad in [
            "",
            "localhost",
            "*",
            "*.com.",
            "a..b",
            "-a.com",
            "a_b.com",
            "a.com/path",
            "http://a.com",
            "10.0.0.1",
            "a.com:0",
            "a.com:99999",
            "*.*.a.com",
            "user@a.com",
        ] {
            assert!(HostPattern::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lists_need_one_to_sixteen_distinct_hosts() -> Result<(), VaultError> {
        assert!(parse_hosts(&[])?.is_empty(), "bound to nowhere yet");
        assert!(parse_hosts(&[" ".into()])?.is_empty());
        let list = parse_hosts(&["a.com".into(), "A.com".into(), "b.com".into()])?;
        assert_eq!(list.len(), 2);
        let many: Vec<String> = (0..17).map(|i| format!("h{i}.example.com")).collect();
        assert!(parse_hosts(&many).is_err());
        Ok(())
    }
}
