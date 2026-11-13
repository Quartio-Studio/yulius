//! The lock-down a browser on a virtual display needs (ADR-0318).
//!
//! On a virtual display a viewer's input is real X input, and the browser runs on the host as
//! the service user. Chrome's own windows (the file chooser of an `<input type=file>`, Save as,
//! printing to a file, DevTools, `file:` and `view-source:` pages) would hand such a viewer the
//! host's files. `--kiosk` hides the toolbar, not these. What switches them off for every tab,
//! frame and window at once is Chrome's managed policy, a file only root writes:
//! [`managed_policy`], which `yulius host setup` installs into [`POLICY_DIRS`].
//!
//! The server cannot read which file a browser loaded (it differs by flavor and packaging), so
//! it asks the browser: the policy blocks a marker address ([`marker_host`]), and a navigation
//! to it either fails as blocked by the administrator or does not
//! ([`crate::BrowserInstance::viewer_input_allowed`]). Viewers' clicks and keys are refused
//! until it does. The marker carries [`POLICY_VERSION`], so a policy file of an older Yulius,
//! without what a newer one added, does not pass.

use serde_json::json;

/// Version of [`managed_policy`]: raised whenever a setting is added, so an older file no
/// longer answers the probe.
pub const POLICY_VERSION: u32 = 1;

/// Name of the policy file in each of [`POLICY_DIRS`].
pub const POLICY_FILE: &str = "yulius.json";

/// The managed policy directories of Google Chrome and of Chromium (Debian's and Ubuntu's
/// packages name theirs differently).
pub const POLICY_DIRS: [&str; 3] = [
    "/etc/opt/chrome/policies/managed",
    "/etc/chromium/policies/managed",
    "/etc/chromium-browser/policies/managed",
];

/// What a navigation the policy blocks fails with.
pub(crate) const BLOCKED: &str = "ERR_BLOCKED_BY_ADMINISTRATOR";

/// The address the probe's hidden tab starts at. The tab is known by the id Chrome gives it,
/// not by this address, which any page can open.
pub(crate) const PROBE_BLANK: &str = "about:blank#yulius-browser-policy";

/// The host [`managed_policy`] blocks so a browser can be asked whether it loaded this version
/// of it. `.invalid` never resolves (RFC 6761): without the policy the navigation fails too,
/// with another error, and nothing leaves the host but a name lookup.
#[must_use]
pub fn marker_host() -> String {
    format!("yulius-browser-policy-v{POLICY_VERSION}.invalid")
}

/// The address the probe navigates to.
pub(crate) fn probe_url() -> String {
    format!("http://{}/", marker_host())
}

/// The managed policy file's content: JSON, one setting a line, ending in a newline.
///
/// - no file selection dialog at all (uploads, Save as, the File System Access pickers);
/// - no downloads, no printing (a print dialog saves to a file);
/// - only web addresses: every address is blocked but `http:`, `https:`, `about:`, `data:`,
///   `blob:` and Chrome's built-in extensions (its PDF viewer). So no `file:`, `view-source:`
///   or `chrome:` page, no `devtools:` page (the developer tools' own window is one), and no
///   address of another program (Chrome would offer to run `xdg-open` on the host for it);
///   and the marker host, which stays blocked as the more specific rule;
/// - the File System Access API answers "not allowed";
/// - no extension can be installed (one could be given access to `file:` addresses).
///
/// Not `DeveloperToolsAvailability`: disallowing the tools also refuses the DevTools protocol
/// Yulius drives the browser with (`Target.attachToTarget` answers "Not allowed", Chrome 157).
#[must_use]
pub fn managed_policy() -> String {
    let policy = json!({
        "AllowFileSelectionDialogs": false,
        "DefaultFileSystemReadGuardSetting": 2,
        "DefaultFileSystemWriteGuardSetting": 2,
        "DownloadRestrictions": 3,
        "ExtensionInstallBlocklist": ["*"],
        "PrintingEnabled": false,
        "URLAllowlist": [
            "http://*",
            "https://*",
            "about:*",
            "data:*",
            "blob:*",
            "chrome-extension://*",
        ],
        "URLBlocklist": ["*", marker_host()],
    });
    let mut text = serde_json::to_string_pretty(&policy).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0318: the file says what it must switch off and blocks the marker of its own
    /// version, which the probe navigates to.
    #[test]
    fn the_policy_switches_off_chromes_own_windows_and_blocks_its_marker() {
        let policy: serde_json::Value = serde_json::from_str(&managed_policy()).unwrap_or_default();
        assert_eq!(policy["AllowFileSelectionDialogs"], false);
        assert!(
            policy.get("DeveloperToolsAvailability").is_none(),
            "it would refuse the DevTools protocol too"
        );
        assert_eq!(policy["DownloadRestrictions"], 3);
        assert_eq!(policy["PrintingEnabled"], false);
        assert_eq!(policy["ExtensionInstallBlocklist"], json!(["*"]));
        let blocked: Vec<&str> = policy["URLBlocklist"]
            .as_array()
            .map(|list| list.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        // Everything but the web: `file:`, `chrome:`, `devtools:`, another program's scheme.
        assert_eq!(blocked, ["*", marker_host().as_str()]);
        let allowed: Vec<&str> = policy["URLAllowlist"]
            .as_array()
            .map(|list| list.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        assert!(allowed.contains(&"http://*") && allowed.contains(&"https://*"));
        assert!(
            allowed.iter().all(|pattern| {
                !pattern.starts_with("file")
                    && !pattern.starts_with("chrome:")
                    && !pattern.starts_with("devtools")
                    && !pattern.starts_with("view-source")
                    && *pattern != "*"
            }),
            "{allowed:?}"
        );
        assert_eq!(
            marker_host(),
            format!("yulius-browser-policy-v{POLICY_VERSION}.invalid")
        );
        assert!(managed_policy().ends_with("}\n"));
        assert_eq!(probe_url(), format!("http://{}/", marker_host()));
    }
}
