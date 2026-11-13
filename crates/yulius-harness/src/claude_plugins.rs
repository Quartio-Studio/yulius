//! The plugins of a Claude Code config dir that bring a language server (SPEC-485, ADR-0327).
//!
//! Claude Code starts a plugin's language server when a session writes a file of the server's
//! language, one server per session, and reports in the terminal when it cannot start. A plugin
//! is switched on in a settings file every session of a home shares, and Claude Code's own dialog
//! installs one with a single Enter. Yulius decides per run instead: the plugins found here are
//! switched off in each run's settings (`disabled_plugins` of a launch request).
//!
//! Claude Code (2.1.323) takes a plugin's `lspServers` from three places: the plugin's entry in
//! its marketplace's `marketplace.json`, the plugin's manifest `.claude-plugin/plugin.json`, and a
//! file `.lsp.json` in the plugin's folder. Their grammar is not mirrored: a key that is there
//! counts, valid or not.
//!
//! The config dir is the agent's, which can put a link, a FIFO or a huge file anywhere in it. As
//! in [`crate::claude_config`] everything is opened one name at a time without following a link
//! (ADR-0174), read up to a bound, and never written. The lookup is tidiness, not a boundary (a
//! language server runs inside the session's containment like any program the agent starts), so
//! what cannot be read counts as bringing nothing and a launch never fails here.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::path::Path;

use nix::fcntl::OFlag;
use nix::sys::stat::Mode;
use serde_json::Value;

use crate::claude_config::{open_dir_at, open_existing_config_dir, read_small};

/// The key a marketplace entry and a plugin manifest name their language servers with.
const LSP_SERVERS: &str = "lspServers";
/// A plugin folder's own file of language servers.
const LSP_FILE: &str = ".lsp.json";
/// The folder of a plugin's manifest and of a marketplace's catalog.
const MANIFEST_DIR: &str = ".claude-plugin";
/// Most installed plugins looked at.
const MAX_PLUGINS: usize = 256;
/// Most version folders of one plugin looked at.
const MAX_VERSIONS: usize = 16;

/// The ids (`name@marketplace`) of the plugins installed in `config_dir` that bring a language
/// server, sorted (SPEC-485 LSP-002). Empty when the config dir or its plugin folder is missing,
/// a link, or unreadable; nothing is created or written.
#[must_use]
pub fn language_server_plugins(config_dir: &Path) -> Vec<String> {
    let Some(plugins) = open_existing_config_dir(config_dir)
        .and_then(|config| open_dir_at(&config, OsStr::new("plugins"), false).ok())
    else {
        return Vec::new();
    };
    let installed: BTreeSet<String> = match json(&plugins, "installed_plugins.json") {
        Some(Value::Object(mut root)) => match root.remove("plugins") {
            Some(Value::Object(installed)) => installed.into_iter().map(|(id, _)| id).collect(),
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let marketplaces = open_dir_at(&plugins, OsStr::new("marketplaces"), false).ok();
    let cache = open_dir_at(&plugins, OsStr::new("cache"), false).ok();
    // A marketplace's catalog is read once, however many of its plugins are installed.
    let mut catalogs: BTreeMap<String, Option<Value>> = BTreeMap::new();
    installed
        .into_iter()
        .take(MAX_PLUGINS)
        .filter(|id| {
            let Some((name, marketplace)) = id.rsplit_once('@') else {
                return false;
            };
            if !plain(name) || !plain(marketplace) {
                return false;
            }
            let catalog = catalogs.entry(marketplace.to_owned()).or_insert_with(|| {
                let dir = open_dir_at(marketplaces.as_ref()?, OsStr::new(marketplace), false);
                let dir = open_dir_at(&dir.ok()?, OsStr::new(MANIFEST_DIR), false).ok()?;
                json(&dir, "marketplace.json")
            });
            in_catalog(catalog.as_ref(), name)
                || cache
                    .as_ref()
                    .is_some_and(|cache| in_folders(cache, marketplace, name))
        })
        .collect()
}

/// Whether the marketplace catalog's entry of the plugin `name` names language servers.
fn in_catalog(catalog: Option<&Value>, name: &str) -> bool {
    catalog
        .and_then(|catalog| catalog.get("plugins"))
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry.get("name").and_then(Value::as_str) == Some(name) && names(entry)
            })
        })
}

/// Whether one of the version folders `cache/<marketplace>/<name>/<version>/` holds `.lsp.json`
/// or a manifest that names language servers.
fn in_folders(cache: &OwnedFd, marketplace: &str, name: &str) -> bool {
    let Some(plugin) = open_dir_at(cache, OsStr::new(marketplace), false)
        .and_then(|dir| open_dir_at(&dir, OsStr::new(name), false))
        .ok()
    else {
        return false;
    };
    versions(&plugin).iter().any(|version| {
        let Ok(folder) = open_dir_at(&plugin, version, false) else {
            return false;
        };
        // The file itself counts, whatever it holds; a link or a FIFO there is not one.
        matches!(read_small(&folder, LSP_FILE), Ok(Some(_)))
            || open_dir_at(&folder, OsStr::new(MANIFEST_DIR), false)
                .ok()
                .and_then(|dir| json(&dir, "plugin.json"))
                .is_some_and(|manifest| names(&manifest))
    })
}

/// Whether the JSON object `value` has `lspServers` with a value other than `null`.
fn names(value: &Value) -> bool {
    value
        .get(LSP_SERVERS)
        .is_some_and(|servers| !servers.is_null())
}

/// Up to [`MAX_VERSIONS`] names in the folder `plugin`, `.` and `..` left out.
fn versions(plugin: &OwnedFd) -> Vec<std::ffi::OsString> {
    use std::os::unix::ffi::OsStrExt as _;
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC;
    let Ok(listing) = nix::dir::Dir::openat(plugin, ".", flags, Mode::empty()) else {
        return Vec::new();
    };
    listing
        .into_iter()
        .filter_map(Result::ok)
        .map(|entry| OsStr::from_bytes(entry.file_name().to_bytes()).to_owned())
        .filter(|name| name != "." && name != "..")
        .take(MAX_VERSIONS)
        .collect()
}

/// The regular file `name` in `dir` as JSON; `None` when it is missing, a link, not a regular
/// file, larger than the bound or not JSON.
fn json(dir: &OwnedFd, name: &str) -> Option<Value> {
    serde_json::from_str(&read_small(dir, name).ok()??).ok()
}

/// Whether `name` is one plain file name: not empty, not `.` or `..`, no `/` and no NUL.
fn plain(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

#[cfg(test)]
mod tests {
    use super::language_server_plugins;
    use serde_json::json;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A config dir `<dir>/home/.claude` with its plugin folder and the installed `ids`.
    fn config_with(dir: &Path, ids: &[&str]) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let config = dir.join("home").join(".claude");
        std::fs::create_dir_all(config.join("plugins"))?;
        let installed: serde_json::Map<String, serde_json::Value> = ids
            .iter()
            .map(|id| {
                (
                    (*id).to_owned(),
                    json!([{ "scope": "user", "version": "1.0.0" }]),
                )
            })
            .collect();
        std::fs::write(
            config.join("plugins").join("installed_plugins.json"),
            json!({ "version": 2, "plugins": installed }).to_string(),
        )?;
        Ok(config)
    }

    /// The marketplace `name`'s catalog with the entries `plugins`.
    fn catalog(config: &Path, name: &str, plugins: serde_json::Value) -> TestResult {
        let dir = config
            .join("plugins/marketplaces")
            .join(name)
            .join(".claude-plugin");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("marketplace.json"),
            json!({ "name": name, "plugins": plugins }).to_string(),
        )?;
        Ok(())
    }

    /// The folder of version `version` of the plugin `id`, made.
    fn folder(
        config: &Path,
        id: &str,
        version: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let (name, marketplace) = id.rsplit_once('@').ok_or("an id")?;
        let dir = config
            .join("plugins/cache")
            .join(marketplace)
            .join(name)
            .join(version);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn manifest(folder: &Path, value: &serde_json::Value) -> TestResult {
        std::fs::create_dir_all(folder.join(".claude-plugin"))?;
        std::fs::write(folder.join(".claude-plugin/plugin.json"), value.to_string())?;
        Ok(())
    }

    const SERVER: &str =
        r#"{"x": {"command": "x-language-server", "extensionToLanguage": {".x": "x"}}}"#;

    /// SPEC-485 LSP-002: each of the three places Claude Code takes a plugin's language servers
    /// from counts, and a plugin without one stays out. The first case is the layout of the
    /// official `rust-analyzer-lsp` plugin: the servers only in the marketplace's entry.
    #[test]
    fn a_plugin_brings_a_language_server_by_its_catalog_entry_its_manifest_or_its_lsp_file()
    -> TestResult {
        let dir = tempfile::tempdir()?;
        let ids = [
            "by-catalog@official",
            "by-manifest@other",
            "by-file@other",
            "skills-only@official",
            "skills-only@other",
        ];
        let config = config_with(dir.path(), &ids)?;
        let servers: serde_json::Value = serde_json::from_str(SERVER)?;
        catalog(
            &config,
            "official",
            json!([
                { "name": "skills-only", "source": "./plugins/skills-only" },
                { "name": "by-catalog", "source": "./plugins/by-catalog", "strict": false,
                  "lspServers": servers },
                // Named in the catalog, not installed: not this home's.
                { "name": "not-installed", "lspServers": servers },
            ]),
        )?;
        catalog(
            &config,
            "other",
            json!([{ "name": "by-manifest" }, { "name": "by-file" }]),
        )?;
        for id in ids {
            let folder = folder(&config, id, "1.0.0")?;
            std::fs::write(folder.join("README.md"), "A plugin.")?;
            match id {
                "by-manifest@other" => manifest(
                    &folder,
                    &json!({ "name": "by-manifest", "lspServers": "./servers.json" }),
                )?,
                "by-file@other" => std::fs::write(folder.join(".lsp.json"), SERVER)?,
                "skills-only@other" => manifest(
                    &folder,
                    &json!({ "name": "skills-only", "lspServers": null, "skills": "./skills" }),
                )?,
                _ => {}
            }
        }
        assert_eq!(
            language_server_plugins(&config),
            ["by-catalog@official", "by-file@other", "by-manifest@other"]
        );
        Ok(())
    }

    /// An older version's folder that still lies in the cache counts too: which one Claude Code
    /// loads is its own choice.
    #[test]
    fn any_version_folder_of_a_plugin_counts() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config_with(dir.path(), &["p@m"])?;
        folder(&config, "p@m", "2.0.0")?;
        assert!(language_server_plugins(&config).is_empty());
        let old = folder(&config, "p@m", "1.0.0")?;
        std::fs::write(old.join(".lsp.json"), SERVER)?;
        assert_eq!(language_server_plugins(&config), ["p@m"]);
        Ok(())
    }

    /// Nothing is created or written, and a home without Claude Code's folders has no plugins.
    #[test]
    fn a_missing_config_dir_has_no_plugins_and_stays_missing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join("home").join(".claude");
        assert!(language_server_plugins(&config).is_empty());
        assert!(!dir.path().join("home").exists(), "nothing is created");
        std::fs::create_dir_all(&config)?;
        assert!(language_server_plugins(&config).is_empty());
        assert_eq!(std::fs::read_dir(&config)?.count(), 0);
        // Files that are not what Claude Code writes bring nothing either.
        std::fs::create_dir_all(config.join("plugins"))?;
        for text in ["", "not json", "[]", r#"{"plugins": []}"#] {
            std::fs::write(config.join("plugins/installed_plugins.json"), text)?;
            assert!(language_server_plugins(&config).is_empty(), "{text:?}");
        }
        Ok(())
    }

    /// ADR-0174: the config dir is the agent's. A link on the way to a file, or in its place, is
    /// never followed (a catalog or a plugin folder outside the home stays unread), and a FIFO
    /// holds nothing.
    #[test]
    fn the_lookup_follows_no_link_and_waits_on_no_fifo() -> TestResult {
        let dir = tempfile::tempdir()?;
        let servers: serde_json::Value = serde_json::from_str(SERVER)?;
        // What a link could reach: a full plugin folder and a catalog outside the home.
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(outside.join(".claude-plugin"))?;
        std::fs::write(outside.join(".lsp.json"), SERVER)?;
        let entries = json!({ "name": "m", "plugins": [{ "name": "p", "lspServers": servers }] });
        std::fs::write(
            outside.join(".claude-plugin/marketplace.json"),
            entries.to_string(),
        )?;
        std::fs::write(outside.join("servers.json"), SERVER)?;

        // The version folder is a link.
        let config = config_with(&dir.path().join("a"), &["p@m"])?;
        std::fs::create_dir_all(config.join("plugins/cache/m/p"))?;
        symlink(&outside, config.join("plugins/cache/m/p/1.0.0"))?;
        assert!(language_server_plugins(&config).is_empty());
        // `.lsp.json` is a link, then a FIFO.
        let config = config_with(&dir.path().join("b"), &["p@m"])?;
        let folder_b = folder(&config, "p@m", "1.0.0")?;
        symlink(outside.join("servers.json"), folder_b.join(".lsp.json"))?;
        assert!(language_server_plugins(&config).is_empty());
        std::fs::remove_file(folder_b.join(".lsp.json"))?;
        nix::unistd::mkfifo(&folder_b.join(".lsp.json"), nix::sys::stat::Mode::S_IRWXU)?;
        assert!(language_server_plugins(&config).is_empty());
        // The marketplace's folder is a link, then its catalog a FIFO.
        let config = config_with(&dir.path().join("c"), &["p@m"])?;
        std::fs::create_dir_all(config.join("plugins/marketplaces"))?;
        symlink(&outside, config.join("plugins/marketplaces/m"))?;
        assert!(language_server_plugins(&config).is_empty());
        std::fs::remove_file(config.join("plugins/marketplaces/m"))?;
        std::fs::create_dir_all(config.join("plugins/marketplaces/m/.claude-plugin"))?;
        nix::unistd::mkfifo(
            &config.join("plugins/marketplaces/m/.claude-plugin/marketplace.json"),
            nix::sys::stat::Mode::S_IRWXU,
        )?;
        assert!(language_server_plugins(&config).is_empty());
        // The plugin folder, the list of installed plugins, the config dir and the home as links.
        let real = config_with(&dir.path().join("d"), &["p@m"])?;
        catalog(&real, "m", json!([{ "name": "p", "lspServers": servers }]))?;
        assert_eq!(language_server_plugins(&real), ["p@m"], "the control");
        let linked = dir.path().join("e/home/.claude");
        std::fs::create_dir_all(&linked)?;
        symlink(real.join("plugins"), linked.join("plugins"))?;
        assert!(language_server_plugins(&linked).is_empty());
        std::fs::remove_file(linked.join("plugins"))?;
        std::fs::create_dir_all(linked.join("plugins"))?;
        symlink(
            real.join("plugins/installed_plugins.json"),
            linked.join("plugins/installed_plugins.json"),
        )?;
        assert!(language_server_plugins(&linked).is_empty());
        std::fs::create_dir_all(dir.path().join("f/home"))?;
        symlink(&real, dir.path().join("f/home/.claude"))?;
        assert!(language_server_plugins(&dir.path().join("f/home/.claude")).is_empty());
        std::fs::create_dir_all(dir.path().join("g"))?;
        symlink(dir.path().join("d/home"), dir.path().join("g/home"))?;
        assert!(language_server_plugins(&dir.path().join("g/home/.claude")).is_empty());
        Ok(())
    }

    /// An id is the agent's text: one that is not `name@marketplace` of two plain file names
    /// opens nothing, so `..` cannot lead the lookup out of the plugin folder.
    #[test]
    fn an_id_that_is_not_two_plain_names_is_skipped() -> TestResult {
        let dir = tempfile::tempdir()?;
        let ids = [
            "../../outside@m",
            "p@../../outside",
            "p@..",
            ".@m",
            "@m",
            "p@",
            "no-marketplace",
            "plain@m",
        ];
        let config = config_with(dir.path(), &ids)?;
        // A folder each of those ids would reach if its parts were joined to a path.
        let outside = config.join("plugins").join("outside");
        std::fs::create_dir_all(outside.join("1.0.0"))?;
        std::fs::write(outside.join("1.0.0/.lsp.json"), SERVER)?;
        std::fs::create_dir_all(config.join("plugins/cache/m"))?;
        std::fs::write(config.join("plugins/.lsp.json"), SERVER)?;
        std::fs::write(config.join("plugins/cache/.lsp.json"), SERVER)?;
        // The control: an id of two plain names with the same kind of folder is found.
        let plain = folder(&config, "plain@m", "1.0.0")?;
        std::fs::write(plain.join(".lsp.json"), SERVER)?;
        assert_eq!(language_server_plugins(&config), ["plain@m"]);
        Ok(())
    }

    /// The bounds: a file larger than 8 MiB is not read, and only the first 16 version folders
    /// and 256 plugins are looked at.
    #[test]
    fn the_lookup_is_bounded() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config_with(dir.path(), &["p@m"])?;
        let folder = folder(&config, "p@m", "1.0.0")?;
        let file = std::fs::File::create(folder.join(".lsp.json"))?;
        file.set_len(8 * 1024 * 1024 + 1)?;
        assert!(language_server_plugins(&config).is_empty(), "oversized");
        file.set_len(8 * 1024 * 1024)?;
        assert_eq!(language_server_plugins(&config), ["p@m"], "at the bound");

        // 300 installed plugins, each with a language server: the first 256 by id.
        let ids: Vec<String> = (0..300).map(|n| format!("p{n:03}@m")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let config = config_with(&dir.path().join("many"), &refs)?;
        let entries: Vec<serde_json::Value> = (0..300)
            .map(|n| json!({ "name": format!("p{n:03}"), "lspServers": {} }))
            .collect();
        catalog(&config, "m", json!(entries))?;
        assert_eq!(language_server_plugins(&config), ids[..256]);

        // 17 version folders of which none but one has a server: at most 16 are opened, so the
        // answer depends on the listing's order, and never fails.
        let config = config_with(&dir.path().join("versions"), &["p@m"])?;
        for n in 0..40 {
            super::tests::folder(&config, "p@m", &format!("{n}.0.0"))?;
        }
        assert!(language_server_plugins(&config).is_empty());
        assert_eq!(
            super::versions(&open(&config.join("plugins/cache/m/p"))?).len(),
            16
        );
        Ok(())
    }

    fn open(path: &Path) -> Result<std::os::fd::OwnedFd, Box<dyn std::error::Error>> {
        Ok(std::fs::File::open(path)?.into())
    }
}
