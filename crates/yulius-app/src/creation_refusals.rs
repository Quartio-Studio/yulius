//! What a person reads when GitHub refuses a repository Yulius creates (SPEC-241 CRT-001,
//! CRT-002): the App permission GitHub asked for, in GitHub's words, or the installation's
//! repository access, each with the GitHub page where an owner changes it.

use crate::context::App;
use crate::error::AppError;

/// Asked for when GitHub names no permission: what it documents for creating a repository.
const CREATE_PERMISSION: &str = "administration=write";

/// The GitHub settings pages of the instance's App.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppPages {
    /// The App's permission settings.
    pub permissions: String,
    /// The App's installation on the owner: accepting new permissions, repository access.
    pub installation: Option<String>,
}

/// The pages of App `slug` owned by `owner` (an organization when `is_org`), with its
/// `installation` there when known. `None` for names a URL cannot safely hold.
pub(crate) fn app_pages(
    owner: &str,
    is_org: bool,
    slug: &str,
    installation: Option<u64>,
) -> Option<AppPages> {
    let plain = |text: &str| {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    if !plain(owner) || !plain(slug) {
        return None;
    }
    let settings = if is_org {
        format!("https://github.com/organizations/{owner}/settings")
    } else {
        "https://github.com/settings".to_owned()
    };
    Some(AppPages {
        permissions: format!("{settings}/apps/{slug}/permissions"),
        installation: installation.map(|id| format!("{settings}/installations/{id}")),
    })
}

/// GitHub's `X-Accepted-GitHub-Permissions` in the words of its settings page:
/// `administration=write; repository_creation=write` reads "Administration: Read and write (or
/// Repository creation: Read and write)". Empty reads as [`CREATE_PERMISSION`].
pub(crate) fn permission_wording(accepted: &str) -> String {
    let accepted = if accepted.trim().is_empty() {
        CREATE_PERMISSION
    } else {
        accepted
    };
    let sets: Vec<String> = accepted
        .split(';')
        .map(|set| {
            set.split(',')
                .filter_map(|permission| {
                    let (name, access) = permission.trim().split_once('=')?;
                    let mut words = name.trim().replace('_', " ");
                    if let Some(first) = words.get(..1) {
                        words.replace_range(..1, &first.to_ascii_uppercase());
                    }
                    let access = match access.trim() {
                        "write" => "Read and write",
                        "read" => "Read-only",
                        "admin" => "Admin",
                        other => other,
                    };
                    Some(format!("{words}: {access}"))
                })
                .collect::<Vec<_>>()
                .join(" and ")
        })
        .filter(|set| !set.is_empty())
        .collect();
    match sets.split_first() {
        None => permission_wording(CREATE_PERMISSION),
        Some((first, [])) => first.clone(),
        Some((first, rest)) => format!("{first} (or {})", rest.join(", or ")),
    }
}

/// CRT-001: GitHub refused to create `owner/name` for a missing App permission.
fn missing_permission(owner: &str, name: &str, accepted: &str, pages: Option<&AppPages>) -> String {
    let permission = permission_wording(accepted);
    let mut text = format!(
        "GitHub refused to create {owner}/{name}: the Yulius GitHub App lacks the repository \
         permission {permission}. Nothing was created. "
    );
    match pages {
        Some(pages) => {
            text.push_str(&format!(
                "An owner of {owner} grants it on {}",
                pages.permissions
            ));
            match &pages.installation {
                Some(installation) => text.push_str(&format!(
                    ", then accepts the new permission on the App's installation at {installation}"
                )),
                None => text.push_str(", then accepts it on the App's installation"),
            }
        }
        None => text.push_str(&format!(
            "An owner of {owner} grants it in the App's settings on GitHub (Permissions), then \
             accepts it on the App's installation"
        )),
    }
    text.push_str("; then create the project again.");
    text
}

/// CRT-002: `owner/name` exists on GitHub, but the App's installation does not include it.
fn unreachable(owner: &str, name: &str, pages: Option<&AppPages>) -> String {
    let place = pages
        .and_then(|pages| pages.installation.as_deref())
        .map_or_else(
            || format!("in {owner}'s GitHub settings, on the Yulius App's installation"),
            |url| format!("at {url}"),
        );
    format!(
        "{owner}/{name} was created on GitHub, but the Yulius GitHub App cannot reach it: its \
         installation covers only selected repositories. Add {name} under Repository access \
         {place} (or choose All repositories), then add {owner}/{name} with Import repository; \
         Yulius gives the empty repository its first commit when onboarding starts."
    )
}

impl App {
    /// The App's settings pages on the instance owner, when they can be told.
    async fn app_pages(&self) -> Option<AppPages> {
        let (owner, is_org) = self.instance_owner().await.ok()?;
        let app = self.github_app().await.ok()?;
        let installation = self.installation().await.ok().map(|(_, id)| id);
        app_pages(&owner, is_org, &app.slug, installation)
    }

    /// The refusal for creating `owner/name` when GitHub answered 403 accepting `accepted`.
    pub(crate) async fn creation_refused(&self, owner: &str, name: &str, accepted: &str) -> AppError {
        let pages = self.app_pages().await;
        AppError::Forbidden(missing_permission(owner, name, accepted, pages.as_ref()))
    }

    /// `err` from setting up the created `owner/name`, or, when the App's installation does not
    /// include it, the refusal that says where to add it (diagnosed only after the failure:
    /// GitHub may list a new repository a moment after creating it).
    pub(crate) async fn created_but_unreachable(
        &self,
        owner: &str,
        name: &str,
        err: AppError,
    ) -> AppError {
        match self.installation_repo(owner, name).await {
            Ok(None) => {
                let pages = self.app_pages().await;
                AppError::Forbidden(unreachable(owner, name, pages.as_ref()))
            }
            _ => err,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_permissions_read_as_on_its_settings_page() {
        assert_eq!(
            permission_wording("administration=write; repository_creation=write"),
            "Administration: Read and write (or Repository creation: Read and write)"
        );
        assert_eq!(
            permission_wording("pull_requests=write,contents=read"),
            "Pull requests: Read and write and Contents: Read-only"
        );
        assert_eq!(permission_wording(""), "Administration: Read and write");
        assert_eq!(permission_wording(" ; "), "Administration: Read and write");
    }

    #[test]
    fn pages_are_the_owners_settings() {
        assert_eq!(
            app_pages("Quartio-Studio", true, "yulius-quartio", Some(7)),
            Some(AppPages {
                permissions:
                    "https://github.com/organizations/Quartio-Studio/settings/apps/yulius-quartio/permissions"
                        .into(),
                installation: Some(
                    "https://github.com/organizations/Quartio-Studio/settings/installations/7"
                        .into()
                ),
            })
        );
        let personal = app_pages("pawel", false, "yulius-pawel", None);
        assert_eq!(
            personal.map(|p| p.permissions),
            Some("https://github.com/settings/apps/yulius-pawel/permissions".into())
        );
        assert_eq!(app_pages("a/b", true, "x", None), None);
        assert_eq!(app_pages("a", true, "", None), None);
    }

    #[test]
    fn a_refusal_names_the_permission_and_both_pages() {
        let pages = app_pages("pawel", false, "yulius-pawel", Some(9));
        let text = missing_permission("pawel", "shop", "administration=write", pages.as_ref());
        assert!(text.contains("Administration: Read and write"), "{text}");
        assert!(text.contains("https://github.com/settings/apps/yulius-pawel/permissions"));
        assert!(text.contains("https://github.com/settings/installations/9"));
        assert!(text.contains("Nothing was created"));
        let text = unreachable("pawel", "shop", pages.as_ref());
        assert!(text.contains("only selected repositories"), "{text}");
        assert!(text.contains("https://github.com/settings/installations/9"));
        assert!(text.contains("Import repository"));
        let text = unreachable("pawel", "shop", None);
        assert!(text.contains("pawel's GitHub settings"), "{text}");
    }
}
