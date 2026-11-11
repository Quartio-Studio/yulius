//! Names of the systemd units around a server unit (ADR-0298): sessions and sandboxes live in a
//! unit of their own, in one slice with the server, so that restarting the server touches none.

/// The suffix every service unit carries.
const SERVICE: &str = ".service";

/// `yulius` of `yulius.service`; `None` for anything but a service unit with a name.
fn stem(service: &str) -> Option<&str> {
    service
        .strip_suffix(SERVICE)
        .filter(|stem| !stem.is_empty())
}

/// The unit that holds the sessions of the server unit `service`.
///
/// ```
/// use yulius_sandbox::units::sessions_unit;
/// assert_eq!(sessions_unit("yulius.service").as_deref(), Some("yulius-sandboxes.service"));
/// assert_eq!(sessions_unit("yulius"), None);
/// ```
#[must_use]
pub fn sessions_unit(service: &str) -> Option<String> {
    stem(service).map(|stem| format!("{stem}-sandboxes{SERVICE}"))
}

/// The root oneshot that moves sessions of the old layout before `service` starts.
///
/// ```
/// use yulius_sandbox::units::adopt_unit;
/// assert_eq!(adopt_unit("yulius.service").as_deref(), Some("yulius-adopt.service"));
/// ```
#[must_use]
pub fn adopt_unit(service: &str) -> Option<String> {
    stem(service).map(|stem| format!("{stem}-adopt{SERVICE}"))
}

/// The slice shared by `service` and its sessions unit. A dash in a slice's name nests it
/// (`a-b.slice` lives in `a.slice`), so everything but letters and digits becomes `_`.
///
/// ```
/// use yulius_sandbox::units::slice;
/// assert_eq!(slice("yulius.service").as_deref(), Some("yulius.slice"));
/// assert_eq!(slice("yulius-dev@2.service").as_deref(), Some("yulius_dev_2.slice"));
/// ```
#[must_use]
pub fn slice(service: &str) -> Option<String> {
    let name: String = stem(service)?
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    Some(format!("{name}.slice"))
}
