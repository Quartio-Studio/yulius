//! Live previews of a sandbox's dev servers in the user's own browser (SPEC-001 BRW-005,
//! BRW-008; SPEC-002 ADR-14).
//!
//! Every (sandbox, port) has its own **preview origin**, a subdomain of the Yulius host:
//! `<scheme>://<port>-<key>.<domain>`, where `key` is a short, stable, opaque label of the
//! sandbox. The interface layer reverse-proxies those hosts to `127.0.0.1:<port>`; this module
//! decides which hosts exist, who may use them and how access is handed over:
//!
//! 1. A signed-in user with write access asks Yulius for a preview
//!    ([`App::preview_authorize`], a top-level or iframe navigation to Yulius). Yulius returns a
//!    **handshake** URL on the preview origin carrying a single-use token (60 s).
//! 2. The preview origin trades the token for a **grant** ([`App::preview_handshake`]): an
//!    opaque random id set as a host-only, `HttpOnly` cookie on that origin only.
//! 3. Every proxied request presents the grant ([`App::preview_access`]); it is re-checked
//!    against the user's Yulius session and sandbox permission at least every 30 s, so signing
//!    out, disabling the user or losing write access ends preview access too.
//!
//! Handshakes and grants live in memory only (bounded): after a restart a preview navigation
//! simply passes through step 1 again. Only the sandbox's own port range is ever reachable, and
//! never a Yulius listener or a sandbox browser's DevTools port (no recursion, BRW-008).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use yulius_contracts::browser::PreviewInfo;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};

/// Lifetime of a handshake token.
pub const HANDSHAKE_TTL_MS: i64 = 60_000;
/// Lifetime of a preview grant (its cookie's `Max-Age`).
pub const GRANT_TTL_MS: i64 = 12 * 3600 * 1000;
/// A grant is re-checked against the session and permissions after this long.
pub const GRANT_RECHECK_MS: i64 = 30_000;
/// Pending handshakes kept at most (all users).
pub const MAX_HANDSHAKES: usize = 1024;
/// Grants kept at most (all users); the oldest are dropped beyond it.
pub const MAX_GRANTS: usize = 4096;
/// Length of a sandbox's preview key (lowercase hex).
pub const KEY_LEN: usize = 12;
/// Path prefix the preview proxy keeps for itself on every preview origin.
pub const RESERVED_PREFIX: &str = "/__yulius/";
/// Longest accepted path to return to after a handshake.
const MAX_RETURN_PATH: usize = 4096;
/// The sandbox key cache is rebuilt from the store at most this often on a miss.
const KEY_REFRESH_MS: i64 = 5_000;

/// Where preview origins live: `<scheme>://<port>-<key>.<domain>[:<port>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewConfig {
    /// `https` (production) or `http` (local development).
    pub scheme: String,
    /// Parent domain of the preview hosts, lowercase (normally the Yulius host).
    pub domain: String,
    /// External port of the preview origins when it is not the scheme's default.
    pub port: Option<u16>,
}

impl PreviewConfig {
    /// Previews under the host of `public_url`, or `None` when that host is an IP address (an
    /// IP has no subdomains) or a bare `localhost` (its subdomains are sites of their own, so
    /// the previews' cookies would be third-party inside Yulius). `https://yulius.example.com`
    /// gives `https://<port>-<key>.yulius.example.com`; `http://yulius.localhost:8080` gives
    /// `http://<port>-<key>.yulius.localhost:8080` (browsers resolve `*.localhost` themselves).
    #[must_use]
    pub fn from_public_url(public_url: &str) -> Option<Self> {
        let (scheme, rest) = public_url.split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return None;
        }
        let authority = rest.split(['/', '?', '#']).next()?;
        if authority.contains('@') || authority.starts_with('[') {
            return None;
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port.parse::<u16>().ok()?)),
            None => (authority, None),
        };
        if host.eq_ignore_ascii_case("localhost") || !host.contains('.') {
            return None;
        }
        Self::new(&scheme, host, port)
    }

    /// A configuration for an explicit domain; `None` for an IP address or an invalid name.
    #[must_use]
    pub fn new(scheme: &str, domain: &str, port: Option<u16>) -> Option<Self> {
        let domain = domain.trim_end_matches('.').to_ascii_lowercase();
        let is_ip = domain.parse::<std::net::IpAddr>().is_ok();
        let valid = !domain.is_empty()
            && domain.len() <= 200
            && domain.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            });
        if is_ip || !valid || (scheme != "https" && scheme != "http") {
            return None;
        }
        let default_port = if scheme == "https" { 443 } else { 80 };
        Some(Self {
            scheme: scheme.to_owned(),
            domain,
            port: port.filter(|p| *p != default_port),
        })
    }

    /// Whether preview cookies carry `Secure` (and the `__Host-` prefix).
    #[must_use]
    pub fn secure(&self) -> bool {
        self.scheme == "https"
    }

    /// `<port>-<key>.<domain>[:<port>]`, the `Host` of a preview.
    #[must_use]
    pub fn host(&self, port: u16, key: &str) -> String {
        match self.port {
            Some(external) => format!("{port}-{key}.{}:{external}", self.domain),
            None => format!("{port}-{key}.{}", self.domain),
        }
    }

    /// The preview origin of `port` in the sandbox with `key`.
    #[must_use]
    pub fn origin(&self, port: u16, key: &str) -> String {
        format!("{}://{}", self.scheme, self.host(port, key))
    }

    /// The origin with `{port}` in place of the port label (for the UI).
    #[must_use]
    pub fn template(&self, key: &str) -> String {
        match self.port {
            Some(external) => format!(
                "{}://{{port}}-{key}.{}:{external}",
                self.scheme, self.domain
            ),
            None => format!("{}://{{port}}-{key}.{}", self.scheme, self.domain),
        }
    }

    /// Parses a `Host` header (or TLS server name) into (port, key). Case-insensitive; the
    /// external port must match when the configuration has one.
    #[must_use]
    pub fn parse_host(&self, host: &str) -> Option<(u16, String)> {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        let (name, port) = match host.rsplit_once(':') {
            Some((name, port)) => (name.to_owned(), Some(port.parse::<u16>().ok()?)),
            None => (host, None),
        };
        let default_port = if self.secure() { 443 } else { 80 };
        let port = port.filter(|p| *p != default_port);
        // Behind a TLS proxy the Host may omit the external port; directly it must match.
        if port.is_some() && port != self.port {
            return None;
        }
        let label = name.strip_suffix(&format!(".{}", self.domain))?;
        let (number, key) = label.split_once('-')?;
        if number.is_empty()
            || number.len() > 5
            || number.starts_with('0')
            || !number.bytes().all(|b| b.is_ascii_digit())
            || key.len() != KEY_LEN
            || !key
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        let number = number.parse::<u16>().ok().filter(|p| *p != 0)?;
        Some((number, key.to_owned()))
    }
}

/// A preview host that exists: the sandbox, the port and the sandbox's other ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewTarget {
    /// The sandbox.
    pub sandbox_id: SandboxId,
    /// The dev-server port on the VM's loopback.
    pub port: u16,
    /// The sandbox's preview key.
    pub key: String,
    /// First port of the sandbox's range.
    pub first: u16,
    /// Last port of the sandbox's range.
    pub last: u16,
    /// Ports in the range that are never previewed (Yulius listeners, DevTools).
    pub excluded: Vec<u16>,
    /// Where preview origins live.
    pub config: PreviewConfig,
}

impl PreviewTarget {
    /// This preview's origin.
    #[must_use]
    pub fn origin(&self) -> String {
        self.config.origin(self.port, &self.key)
    }

    /// This preview's `Host`.
    #[must_use]
    pub fn host(&self) -> String {
        self.config.host(self.port, &self.key)
    }

    /// Whether `port` of this sandbox may be previewed.
    #[must_use]
    pub fn allows_port(&self, port: u16) -> bool {
        (self.first..=self.last).contains(&port) && !self.excluded.contains(&port)
    }

    /// The preview origin of another port of the same sandbox.
    #[must_use]
    pub fn origin_of(&self, port: u16) -> Option<String> {
        self.allows_port(port)
            .then(|| self.config.origin(port, &self.key))
    }

    /// The port behind a preview origin of this sandbox (this one or a sibling).
    #[must_use]
    pub fn port_of_origin(&self, origin: &str) -> Option<u16> {
        let origin = origin.trim_end_matches('/').to_ascii_lowercase();
        let (scheme, host) = origin.split_once("://")?;
        if scheme != self.config.scheme {
            return None;
        }
        let (port, key) = self.config.parse_host(host)?;
        (key == self.key && self.allows_port(port)).then_some(port)
    }
}

/// Why a preview host or request is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewRefusal {
    /// Previews are not configured on this instance.
    Disabled,
    /// No sandbox has this preview host.
    UnknownHost,
    /// The port is outside the sandbox's range.
    OutsideRange {
        /// First port of the range.
        first: u16,
        /// Last port of the range.
        last: u16,
    },
    /// The port belongs to Yulius itself (a listener or a sandbox browser's DevTools).
    Yulius,
}

impl PreviewRefusal {
    /// Text safe to show to the user.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => "live previews are not configured on this server".into(),
            Self::UnknownHost => "no sandbox has this preview address".into(),
            Self::OutsideRange { first, last } => {
                format!("only this sandbox's ports {first}-{last} can be previewed")
            }
            Self::Yulius => {
                "this port belongs to Yulius itself, which cannot be opened inside its own browser"
                    .into()
            }
        }
    }
}

/// What a preview request may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewAccess {
    /// The grant is valid for this preview.
    Allowed,
    /// No grant, an expired or revoked one, or one for another preview.
    Denied,
}

#[derive(Debug, Clone)]
struct Pass {
    host: String,
    sandbox: SandboxId,
    port: u16,
    user: UserId,
    session_hash: Option<String>,
    created: i64,
    expires: i64,
    checked: i64,
}

/// In-memory preview handshakes, grants and the sandbox key cache.
#[derive(Debug, Default)]
pub(crate) struct PreviewState {
    handshakes: Mutex<HashMap<String, Pass>>,
    grants: Mutex<HashMap<String, Pass>>,
    keys: Mutex<KeyCache>,
}

#[derive(Debug, Default)]
struct KeyCache {
    by_key: HashMap<String, SandboxId>,
    refreshed_at: Option<i64>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A path safe to redirect to on the preview origin: absolute, never protocol-relative and
/// without control characters; anything else becomes `/`.
#[must_use]
pub fn safe_return_path(to: &str) -> String {
    let ok = to.starts_with('/')
        && !to.starts_with("//")
        && !to.starts_with("/\\")
        && to.len() <= MAX_RETURN_PATH
        && !to.starts_with(RESERVED_PREFIX)
        && !to.chars().any(|c| c.is_control() || c == '\\' || c == ' ');
    if ok { to.to_owned() } else { "/".to_owned() }
}

fn random_hex(app: &App) -> String {
    app.deps
        .crypto
        .random_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl App {
    /// Where previews live, when they are enabled.
    #[must_use]
    pub fn preview_config(&self) -> Option<&PreviewConfig> {
        self.config.preview.as_ref()
    }

    /// The stable, opaque preview key of a sandbox.
    #[must_use]
    pub fn preview_key(&self, sandbox: &SandboxId) -> String {
        let digest = self
            .deps
            .crypto
            .sha256_hex(format!("yulius-preview:{sandbox}").as_bytes());
        digest.chars().take(KEY_LEN).collect()
    }

    /// Whether `host` has the shape of a preview host of this instance (no lookup).
    #[must_use]
    pub fn is_preview_host(&self, host: &str) -> bool {
        self.preview_config()
            .is_some_and(|config| config.parse_host(host).is_some())
    }

    /// Ports that are never reachable through a preview: Yulius's own listeners and the
    /// sandbox browsers' DevTools endpoints.
    pub async fn preview_excluded_ports(&self) -> Vec<u16> {
        let mut ports = self.config.own_ports.clone();
        ports.extend(self.deps.browser.devtools_ports().await);
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    async fn sandbox_for_key(&self, key: &str) -> AppResult<Option<SandboxId>> {
        if let Some(found) = lock(&self.previews.keys).by_key.get(key).cloned() {
            return Ok(Some(found));
        }
        let now = self.now().0;
        {
            let cache = lock(&self.previews.keys);
            if cache
                .refreshed_at
                .is_some_and(|at| now.saturating_sub(at) < KEY_REFRESH_MS)
            {
                return Ok(None);
            }
        }
        let sandboxes = self.deps.store.sandboxes().await?;
        let by_key: HashMap<String, SandboxId> = sandboxes
            .into_iter()
            .filter(|s| s.removed_at.is_none())
            .map(|s| (self.preview_key(&s.id), s.id))
            .collect();
        let found = by_key.get(key).cloned();
        let mut cache = lock(&self.previews.keys);
        cache.by_key = by_key;
        cache.refreshed_at = Some(now);
        Ok(found)
    }

    /// Resolves a preview host to its sandbox and port.
    ///
    /// # Errors
    /// [`PreviewRefusal`] when previews are off, the host names no sandbox, or the port is
    /// outside the sandbox's range or belongs to Yulius.
    pub async fn preview_target(&self, host: &str) -> Result<PreviewTarget, PreviewRefusal> {
        let config = self.preview_config().ok_or(PreviewRefusal::Disabled)?;
        let (port, key) = config.parse_host(host).ok_or(PreviewRefusal::UnknownHost)?;
        let sandbox_id = self
            .sandbox_for_key(&key)
            .await
            .ok()
            .flatten()
            .ok_or(PreviewRefusal::UnknownHost)?;
        let record = self
            .deps
            .store
            .sandbox(&sandbox_id)
            .await
            .ok()
            .flatten()
            .filter(|s| s.removed_at.is_none())
            .ok_or(PreviewRefusal::UnknownHost)?;
        let first = record.port_base;
        let last = first.saturating_add(record.port_count.saturating_sub(1));
        if record.port_count == 0 || !(first..=last).contains(&port) {
            return Err(PreviewRefusal::OutsideRange { first, last });
        }
        let excluded = self.preview_excluded_ports().await;
        if excluded.contains(&port) {
            return Err(PreviewRefusal::Yulius);
        }
        Ok(PreviewTarget {
            sandbox_id,
            port,
            key,
            first,
            last,
            excluded,
            config: config.clone(),
        })
    }

    /// The preview origins of a sandbox for the browser panel, with its listening ports.
    ///
    /// # Errors
    /// `NotFound` for invisible sandboxes, `Forbidden` without write access.
    pub async fn preview_info(&self, ctx: &AuthContext, sandbox: &str) -> AppResult<PreviewInfo> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        let (_, first, last, listening) = self.sandbox_listening_ports(sandbox).await?;
        let excluded: Vec<u16> = self
            .preview_excluded_ports()
            .await
            .into_iter()
            .filter(|p| (first..=last).contains(p))
            .collect();
        let (enabled, reason, template) = match self.preview_config() {
            Some(config) => (
                true,
                None,
                Some(config.template(&self.preview_key(&sandbox_id))),
            ),
            None => (
                false,
                Some(
                    "live previews need the public URL to be a domain name (not an IP address or a bare localhost) with wildcard DNS and TLS for its subdomains"
                        .into(),
                ),
                None,
            ),
        };
        Ok(PreviewInfo {
            enabled,
            reason,
            origin_template: template,
            first_port: first,
            last_port: last,
            listening,
            excluded,
        })
    }

    /// Step 1 of the handshake: a signed-in user with write access asks for `host`; returns the
    /// handshake URL on the preview origin (single use, [`HANDSHAKE_TTL_MS`]).
    ///
    /// # Errors
    /// `NotFound` for unknown hosts, refused ports and invisible sandboxes, `Forbidden` without
    /// write access, `Capacity` when too many handshakes are pending.
    pub async fn preview_authorize(
        &self,
        ctx: &AuthContext,
        host: &str,
        to: &str,
    ) -> AppResult<String> {
        let target = self.preview_target(host).await.map_err(|refusal| {
            if refusal == PreviewRefusal::UnknownHost || refusal == PreviewRefusal::Disabled {
                AppError::NotFound
            } else {
                AppError::Forbidden(refusal.message())
            }
        })?;
        self.require(&ctx.user, &target.sandbox_id, Action::Work)
            .await?;
        let now = self.now().0;
        let token = random_hex(self);
        {
            let mut pending = lock(&self.previews.handshakes);
            pending.retain(|_, pass| pass.expires > now);
            if pending.len() >= MAX_HANDSHAKES {
                return Err(AppError::Capacity(
                    "too many preview sign-ins are pending; try again in a minute".into(),
                ));
            }
            pending.insert(
                token.clone(),
                Pass {
                    host: target.host(),
                    sandbox: target.sandbox_id.clone(),
                    port: target.port,
                    user: ctx.user.id.clone(),
                    session_hash: ctx.session_hash.clone(),
                    created: now,
                    expires: now.saturating_add(HANDSHAKE_TTL_MS),
                    checked: now,
                },
            );
        }
        let to = safe_return_path(to);
        Ok(format!(
            "{}/__yulius/handshake?t={token}&to={}",
            target.origin(),
            crate::browser::percent_encode(&to, true)
        ))
    }

    /// Step 2: trades a handshake token presented on `host` for a grant id (the preview
    /// cookie's value). The token works once, only on the host it was made for.
    ///
    /// # Errors
    /// `Forbidden` for unknown, used, expired or foreign tokens.
    pub fn preview_handshake(&self, host: &str, token: &str) -> AppResult<String> {
        let now = self.now().0;
        let denied = || AppError::Forbidden("this preview link expired; open it again".into());
        let pass = lock(&self.previews.handshakes)
            .remove(token)
            .ok_or_else(denied)?;
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if pass.expires <= now || !same_host(&pass.host, &host) {
            return Err(denied());
        }
        let grant = random_hex(self);
        let mut grants = lock(&self.previews.grants);
        grants.retain(|_, pass| pass.expires > now);
        while grants.len() >= MAX_GRANTS {
            let Some(oldest) = grants
                .iter()
                .min_by_key(|(_, pass)| pass.created)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            grants.remove(&oldest);
        }
        grants.insert(
            grant.clone(),
            Pass {
                created: now,
                expires: now.saturating_add(GRANT_TTL_MS),
                checked: now,
                ..pass
            },
        );
        Ok(grant)
    }

    /// Step 3: whether a request with grant `grant` may reach `target`. Re-checks the user's
    /// session and write access after [`GRANT_RECHECK_MS`]; a failed check revokes the grant.
    pub async fn preview_access(&self, target: &PreviewTarget, grant: &str) -> PreviewAccess {
        let now = self.now().0;
        let pass = {
            let grants = lock(&self.previews.grants);
            grants.get(grant).cloned()
        };
        let Some(pass) = pass else {
            return PreviewAccess::Denied;
        };
        if pass.expires <= now
            || pass.sandbox != target.sandbox_id
            || pass.port != target.port
            || !same_host(&pass.host, &target.host())
        {
            return PreviewAccess::Denied;
        }
        if now.saturating_sub(pass.checked) < GRANT_RECHECK_MS {
            return PreviewAccess::Allowed;
        }
        if self.preview_pass_still_valid(&pass).await {
            if let Some(stored) = lock(&self.previews.grants).get_mut(grant) {
                stored.checked = now;
            }
            PreviewAccess::Allowed
        } else {
            lock(&self.previews.grants).remove(grant);
            PreviewAccess::Denied
        }
    }

    /// Whether a grant still holds now, re-checked regardless of its last check (open
    /// WebSockets through a preview call this periodically).
    pub async fn preview_grant_still_valid(&self, grant: &str) -> bool {
        let pass = lock(&self.previews.grants).get(grant).cloned();
        match pass {
            Some(pass) if pass.expires > self.now().0 => {
                let valid = self.preview_pass_still_valid(&pass).await;
                if !valid {
                    lock(&self.previews.grants).remove(grant);
                }
                valid
            }
            _ => false,
        }
    }

    async fn preview_pass_still_valid(&self, pass: &Pass) -> bool {
        let Ok(Some(user)) = self.deps.store.user(&pass.user).await else {
            return false;
        };
        if user.disabled_at.is_some() {
            return false;
        }
        if let Some(hash) = &pass.session_hash {
            let live = matches!(
                self.deps.store.web_session(hash).await,
                Ok(Some(session)) if session.revoked_at.is_none() && session.expires_at > self.now()
            );
            if !live {
                return false;
            }
        }
        self.require(&user, &pass.sandbox, Action::Work)
            .await
            .is_ok()
    }

    /// The `ask` endpoint of an on-demand TLS proxy: whether `domain` is a preview host of an
    /// existing sandbox port (certificates are issued only for those).
    pub async fn preview_tls_allowed(&self, domain: &str) -> bool {
        self.preview_target(domain).await.is_ok()
    }

    /// Grants currently held (tests and diagnostics).
    #[must_use]
    pub fn preview_grant_count(&self) -> usize {
        let now = self.now().0;
        lock(&self.previews.grants)
            .values()
            .filter(|pass| pass.expires > now)
            .count()
    }
}

/// Hosts compared without a default port.
fn same_host(a: &str, b: &str) -> bool {
    let strip = |h: &str| {
        let h = h.trim_end_matches('.').to_ascii_lowercase();
        h.strip_suffix(":443")
            .or_else(|| h.strip_suffix(":80"))
            .map_or_else(|| h.clone(), str::to_owned)
    };
    strip(a) == strip(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn https() -> PreviewConfig {
        PreviewConfig {
            scheme: "https".into(),
            domain: "109-199-123-240.sslip.io".into(),
            port: None,
        }
    }

    #[test]
    fn configuration_follows_the_public_host_and_refuses_ip_addresses() {
        assert_eq!(
            PreviewConfig::from_public_url("https://Yulius.Example.com/"),
            Some(PreviewConfig {
                scheme: "https".into(),
                domain: "yulius.example.com".into(),
                port: None
            })
        );
        assert_eq!(
            PreviewConfig::from_public_url("http://yulius.localhost:8080"),
            Some(PreviewConfig {
                scheme: "http".into(),
                domain: "yulius.localhost".into(),
                port: Some(8080)
            })
        );
        assert_eq!(
            PreviewConfig::from_public_url("https://yulius.example.com:443").map(|c| c.port),
            Some(None)
        );
        for url in [
            "http://127.0.0.1:8080",
            "https://[::1]:8443",
            "https://10.0.0.1",
            "ftp://host",
            "https://user@host",
            "https://bad_host.example",
            "http://localhost:8080",
            "http://intranet:8080",
        ] {
            assert_eq!(PreviewConfig::from_public_url(url), None, "{url}");
        }
    }

    #[test]
    fn hosts_round_trip_and_reject_lookalikes() {
        let config = https();
        assert_eq!(
            PreviewConfig::from_public_url("https://109-199-123-240.sslip.io"),
            Some(config.clone())
        );
        let host = config.host(20001, "0123456789ab");
        assert_eq!(host, "20001-0123456789ab.109-199-123-240.sslip.io");
        assert_eq!(
            config.origin(20001, "0123456789ab"),
            "https://20001-0123456789ab.109-199-123-240.sslip.io"
        );
        assert_eq!(
            config.parse_host(&host),
            Some((20001, "0123456789ab".into()))
        );
        assert_eq!(
            config.parse_host("20001-0123456789AB.109-199-123-240.SSLIP.IO:443"),
            Some((20001, "0123456789ab".into()))
        );
        for bad in [
            "109-199-123-240.sslip.io",
            "x.20001-0123456789ab.109-199-123-240.sslip.io",
            "20001-0123456789ab.evil.109-199-123-240.sslip.io.attacker.io",
            "20001-0123456789ab.109-199-123-240.sslip.io:8443",
            "020001-0123456789ab.109-199-123-240.sslip.io",
            "0-0123456789ab.109-199-123-240.sslip.io",
            "70000-0123456789ab.109-199-123-240.sslip.io",
            "20001-0123456789a.109-199-123-240.sslip.io",
            "20001-0123456789ag.109-199-123-240.sslip.io",
            "20001_0123456789ab.109-199-123-240.sslip.io",
            "20001-0123456789ab109-199-123-240.sslip.io",
        ] {
            assert_eq!(config.parse_host(bad), None, "{bad}");
        }
        let local = PreviewConfig::from_public_url("http://yulius.localhost:8080");
        assert_eq!(
            local.as_ref().map(|c| c.origin(20001, "0123456789ab")),
            Some("http://20001-0123456789ab.yulius.localhost:8080".into())
        );
        assert_eq!(
            local
                .as_ref()
                .and_then(|c| c.parse_host("20001-0123456789ab.yulius.localhost:8080")),
            Some((20001, "0123456789ab".into()))
        );
        assert_eq!(
            local.and_then(|c| c.parse_host("20001-0123456789ab.yulius.localhost:9090")),
            None
        );
    }

    #[test]
    fn targets_know_their_siblings_and_exclusions() -> Result<(), yulius_domain::ids::IdError> {
        let target = PreviewTarget {
            sandbox_id: SandboxId::parse("s1")?,
            port: 20001,
            key: "0123456789ab".into(),
            first: 20000,
            last: 20019,
            excluded: vec![20005],
            config: https(),
        };
        assert_eq!(
            target.origin_of(20002),
            Some("https://20002-0123456789ab.109-199-123-240.sslip.io".into())
        );
        assert_eq!(target.origin_of(20005), None);
        assert_eq!(target.origin_of(3000), None);
        assert_eq!(
            target.port_of_origin("https://20002-0123456789ab.109-199-123-240.sslip.io"),
            Some(20002)
        );
        assert_eq!(
            target.port_of_origin("http://20002-0123456789ab.109-199-123-240.sslip.io"),
            None
        );
        assert_eq!(
            target.port_of_origin("https://20002-ffffffffffff.109-199-123-240.sslip.io"),
            None
        );
        assert_eq!(target.port_of_origin("null"), None);
        Ok(())
    }

    #[test]
    fn return_paths_stay_on_the_preview_origin() {
        assert_eq!(safe_return_path("/app?x=1#y"), "/app?x=1#y");
        for bad in [
            "//evil.example/",
            "/\\evil.example",
            "https://evil.example/",
            "javascript:alert(1)",
            "/a\nb",
            "/__yulius/handshake?t=x",
            "",
        ] {
            assert_eq!(safe_return_path(bad), "/", "{bad:?}");
        }
    }
}
