//! The plugins of a Claude Code session that bring a language server (SPEC-485, ADR-0327).
//!
//! Claude Code starts a plugin's language server when a session writes a file of the server's
//! language, one server per session, and reports in the terminal when it cannot start. A plugin
//! is switched on in a settings file every session of a home shares, and Claude Code's own dialog
//! installs one with a single Enter. Yulius decides per run instead: the plugins found here are
//! switched off in each run's settings (`disabled_plugins` of a launch request).
//!
//! What Claude Code (2.1.323) loads, as measured with the real tool, and what is read here:
//! - **Which plugins.** The ones installed (`plugins/installed_plugins.json`) and the ones a
//!   settings file switches on without an install record, which Claude Code loads in place from a
//!   marketplace that is a folder: the home's `settings.json`, the worktree's
//!   `.claude/settings.json` and `.claude/settings.local.json`.
//! - **Where a marketplace lies.** Where `plugins/known_marketplaces.json` says
//!   (`installLocation`: a clone below `plugins/marketplaces`, or a folder anywhere, such as one a
//!   repository names in `extraKnownMarketplaces`, which Claude Code registers there at a
//!   session's start), else `plugins/marketplaces/<marketplace>`.
//! - **Where a plugin's `lspServers` come from.** Its entry in the marketplace's
//!   `marketplace.json`; its manifest `.claude-plugin/plugin.json`; a file `.lsp.json` in its
//!   folder. Its folders: the installed copies `plugins/cache/<marketplace>/<name>/<version>`,
//!   the install record's `installPath`, and the entry's relative `source` in the marketplace.
//!
//! Their grammar is not mirrored: a key that is there counts, valid or not.
//!
//! All of it is the agent's, which can put a link, a FIFO or a huge file anywhere. As in
//! [`crate::claude_config`] everything is opened one name at a time without following a link, from
//! a folder the agent cannot replace (ADR-0174); a path the agent wrote is opened only below such
//! a folder. Nothing is written, and the whole lookup has one budget of bytes and folders, so a
//! launch waits a moment at most. The lookup is tidiness, not a boundary (a language server runs
//! inside the session's containment like any program the agent starts): what cannot be read counts
//! as bringing nothing, a launch never fails here, and what was left unread is said.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::{Component, Path};

use nix::fcntl::{AtFlags, OFlag};
use nix::sys::stat::{Mode, SFlag, fstatat};
use serde_json::Value;

use crate::claude_config::{open_dir_at, open_regular};

/// The key a marketplace entry and a plugin manifest name their language servers with.
const LSP_SERVERS: &str = "lspServers";
/// A plugin folder's own file of language servers.
const LSP_FILE: &str = ".lsp.json";
/// The folder of a plugin's manifest and of a marketplace's catalog.
const MANIFEST_DIR: &str = ".claude-plugin";
/// The settings key of the plugins a settings file switches on or off.
const ENABLED_PLUGINS: &str = "enabledPlugins";
/// Most plugins looked at.
const MAX_PLUGINS: usize = 256;
/// Longest plugin id taken. With [`MAX_PLUGINS`] it bounds what a run's settings grow by (about
/// 54 kB), far below the size one argument of a command line may have.
const MAX_ID_BYTES: usize = 200;
/// Most version folders of one plugin looked at.
const MAX_VERSIONS: usize = 16;
/// Largest file read. The official marketplace's catalog has 190 kB.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Most bytes read in one lookup, all files together.
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
/// Most plugin folders looked into in one lookup.
const MAX_FOLDERS: usize = 1024;
/// Most notes about what was left unread.
const MAX_NOTES: usize = 8;

/// What [`language_server_plugins`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageServerPlugins {
    /// The ids (`name@marketplace`) of the plugins that bring a language server, sorted.
    pub plugins: Vec<String>,
    /// What was there and could not be read, or was left out by a bound, in a few words each
    /// (at most 8): a plugin behind it is not known to bring no language server.
    pub unread: Vec<String>,
}

/// The plugins a Claude Code session with the config dir `config_dir`, started in `worktree`,
/// would load a language server from (SPEC-485 LSP-002). `sandbox_dir` is the folder the session's
/// own folders lie in and the agent cannot replace (a task's sandbox directory): a marketplace or
/// an installed copy outside the config dir is looked at only below it or below the folder above
/// the home. Nothing found for a config dir that is missing, a link or unreadable; nothing is
/// created or written.
#[must_use]
pub fn language_server_plugins(
    config_dir: &Path,
    sandbox_dir: &Path,
    worktree: &Path,
) -> LanguageServerPlugins {
    let mut lookup = Lookup::new(config_dir, sandbox_dir);
    let plugins = lookup.find(config_dir, worktree);
    LanguageServerPlugins {
        plugins,
        unread: lookup.unread,
    }
}

/// One lookup: its budget, the folders the agent cannot replace, and what it left unread.
struct Lookup<'a> {
    bytes_left: u64,
    folders_left: usize,
    /// The folder above the home and the sandbox directory.
    anchors: Vec<&'a Path>,
    unread: Vec<String>,
}

/// A marketplace as far as it is needed: its folder, and of the plugins asked about the ones whose
/// entry names language servers and each one's folder in the marketplace.
#[derive(Default)]
struct Marketplace {
    root: Option<OwnedFd>,
    with_servers: BTreeSet<String>,
    sources: BTreeMap<String, String>,
}

impl<'a> Lookup<'a> {
    fn new(config_dir: &'a Path, sandbox_dir: &'a Path) -> Self {
        let above_home = config_dir.parent().and_then(Path::parent);
        Self {
            bytes_left: MAX_TOTAL_BYTES,
            folders_left: MAX_FOLDERS,
            anchors: above_home.into_iter().chain([sandbox_dir]).collect(),
            unread: Vec::new(),
        }
    }

    fn find(&mut self, config_dir: &Path, worktree: &Path) -> Vec<String> {
        let Some(config) = crate::claude_config::open_existing_config_dir(config_dir) else {
            return Vec::new();
        };
        let plugins = open_dir_at(&config, OsStr::new("plugins"), false).ok();
        // The candidates: installed, or switched on by a settings file of this session.
        let mut copies: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Some(Value::Object(installed)) = plugins
            .as_ref()
            .and_then(|plugins| self.json(plugins, "installed_plugins.json"))
            .and_then(|mut root| root.get_mut("plugins").map(Value::take))
        {
            for (id, records) in installed {
                let paths = records
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|record| record.get("installPath")?.as_str())
                    .take(MAX_VERSIONS)
                    .map(str::to_owned)
                    .collect();
                copies.insert(id, paths);
            }
        }
        let mut ids: BTreeSet<String> = copies.keys().cloned().collect();
        ids.extend(self.switched_on(&config, "settings.json"));
        if let Some(project) = self
            .below_anchor(worktree)
            .and_then(|worktree| open_dir_at(&worktree, OsStr::new(".claude"), false).ok())
        {
            ids.extend(self.switched_on(&project, "settings.json"));
            ids.extend(self.switched_on(&project, "settings.local.json"));
        }
        let (taken, long): (Vec<String>, Vec<String>) =
            ids.into_iter().partition(|id| id.len() <= MAX_ID_BYTES);
        if !long.is_empty() {
            self.note(format!(
                "{} plugin ids longer than {MAX_ID_BYTES} bytes",
                long.len()
            ));
        }
        if taken.len() > MAX_PLUGINS {
            self.note(format!(
                "{} plugins beyond the first {MAX_PLUGINS}",
                taken.len() - MAX_PLUGINS
            ));
        }
        let Some(plugins) = plugins else {
            // Without a plugin folder nothing is installed and no marketplace is known.
            return Vec::new();
        };
        let candidates: Vec<(String, String, String)> = taken
            .into_iter()
            .take(MAX_PLUGINS)
            .filter_map(|id| {
                let (name, marketplace) = id.rsplit_once('@')?;
                (plain(name) && plain(marketplace))
                    .then(|| (id.clone(), name.to_owned(), marketplace.to_owned()))
            })
            .collect();
        let marketplaces = self.marketplaces(&plugins, &candidates);
        let cache = open_dir_at(&plugins, OsStr::new("cache"), false).ok();
        candidates
            .into_iter()
            .filter(|(id, name, marketplace)| {
                let market = marketplaces.get(marketplace);
                if market.is_some_and(|market| market.with_servers.contains(name)) {
                    return true;
                }
                // The plugin's folders: its place in the marketplace, the installed copies in the
                // cache, and where its install records say a copy lies.
                let mut folders: Vec<OwnedFd> = Vec::new();
                if let Some((root, source)) = market
                    .and_then(|market| Some((market.root.as_ref()?, market.sources.get(name)?)))
                {
                    folders.extend(walk(root, source));
                }
                if let Some(versions) = cache.as_ref().and_then(|cache| {
                    open_dir_at(cache, OsStr::new(marketplace), false)
                        .and_then(|dir| open_dir_at(&dir, OsStr::new(name), false))
                        .ok()
                }) {
                    folders.extend(
                        names_in(&versions)
                            .iter()
                            .filter_map(|version| open_dir_at(&versions, version, false).ok()),
                    );
                }
                for path in copies.get(id).into_iter().flatten() {
                    folders.extend(self.below_anchor(Path::new(path)));
                }
                folders.iter().any(|folder| self.brings(folder))
            })
            .map(|(id, _, _)| id)
            .collect()
    }

    /// The marketplaces of `candidates`, each read once: where `known_marketplaces.json` says it
    /// lies when that can be opened, else `plugins/marketplaces/<marketplace>`.
    fn marketplaces(
        &mut self,
        plugins: &OwnedFd,
        candidates: &[(String, String, String)],
    ) -> BTreeMap<String, Marketplace> {
        let known = self.json(plugins, "known_marketplaces.json");
        let cloned = open_dir_at(plugins, OsStr::new("marketplaces"), false).ok();
        let mut wanted: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (_, name, marketplace) in candidates {
            wanted.entry(marketplace).or_default().insert(name);
        }
        let mut marketplaces = BTreeMap::new();
        for (marketplace, names) in wanted {
            let said = known
                .as_ref()
                .and_then(|known| known.get(marketplace)?.get("installLocation")?.as_str())
                .and_then(|location| self.below_anchor(Path::new(location)));
            let root =
                said.or_else(|| open_dir_at(cloned.as_ref()?, OsStr::new(marketplace), false).ok());
            let mut market = Marketplace::default();
            if let Some(catalog) = root
                .as_ref()
                .and_then(|root| open_dir_at(root, OsStr::new(MANIFEST_DIR), false).ok())
                .and_then(|dir| self.json(&dir, "marketplace.json"))
            {
                // A bare source name lies below `metadata.pluginRoot`, a `./path` below the root.
                let plugin_root = catalog
                    .get("metadata")
                    .and_then(|metadata| metadata.get("pluginRoot")?.as_str())
                    .map(str::to_owned);
                for entry in catalog
                    .get("plugins")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(name) = entry.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    if !names.contains(name) {
                        continue;
                    }
                    if has_servers(entry) {
                        market.with_servers.insert(name.to_owned());
                    }
                    match (entry.get("source").and_then(Value::as_str), &plugin_root) {
                        (Some(source), _) if source.starts_with("./") => {
                            market.sources.insert(name.to_owned(), source.to_owned());
                        }
                        (Some(source), Some(plugin_root)) if plain(source) => {
                            market
                                .sources
                                .insert(name.to_owned(), format!("{plugin_root}/{source}"));
                        }
                        _ => {}
                    }
                }
            }
            market.root = root;
            marketplaces.insert(marketplace.to_owned(), market);
        }
        marketplaces
    }

    /// The ids the settings file `name` in `dir` switches on: the keys of its `enabledPlugins`
    /// whose value is not `false`.
    fn switched_on(&mut self, dir: &OwnedFd, name: &str) -> Vec<String> {
        match self
            .json(dir, name)
            .and_then(|mut settings| settings.get_mut(ENABLED_PLUGINS).map(Value::take))
        {
            Some(Value::Object(enabled)) => enabled
                .into_iter()
                .filter(|(_, value)| *value != Value::Bool(false))
                .map(|(id, _)| id)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether the plugin folder `folder` holds `.lsp.json` (a regular file, whatever it holds)
    /// or a manifest that names language servers. Counts against the folder budget.
    fn brings(&mut self, folder: &OwnedFd) -> bool {
        if self.folders_left == 0 {
            self.note(format!("plugin folders beyond the first {MAX_FOLDERS}"));
            return false;
        }
        self.folders_left -= 1;
        let is_file = |stat: nix::sys::stat::FileStat| {
            SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG
        };
        fstatat(folder, LSP_FILE, AtFlags::AT_SYMLINK_NOFOLLOW).is_ok_and(is_file)
            || open_dir_at(folder, OsStr::new(MANIFEST_DIR), false)
                .ok()
                .and_then(|dir| self.json(&dir, "plugin.json"))
                .is_some_and(|manifest| has_servers(&manifest))
    }

    /// The regular file `name` in `dir` as JSON, within the budget. `None`, silently, when it is
    /// missing; `None` with a note when it is a link, not a regular file, too large, past the
    /// budget or not JSON.
    fn json(&mut self, dir: &OwnedFd, name: &str) -> Option<Value> {
        let file = match open_regular(dir, name) {
            Ok(Some(file)) => file,
            Ok(None) => return None,
            Err(error) => {
                self.note(error.to_string());
                return None;
            }
        };
        let size = file.metadata().map_or(u64::MAX, |metadata| metadata.len());
        if size > MAX_FILE_BYTES || size > self.bytes_left {
            self.note(format!(
                "{name} is too large ({size} bytes) or past the lookup's budget"
            ));
            return None;
        }
        let mut text = String::new();
        // The file may have grown since the `stat`: read no further than the bound.
        let read = file.take(MAX_FILE_BYTES).read_to_string(&mut text);
        self.bytes_left = self.bytes_left.saturating_sub(text.len() as u64);
        let value = read.ok().and_then(|_| serde_json::from_str(&text).ok());
        if value.is_none() {
            self.note(format!("{name} is not JSON"));
        }
        value
    }

    /// The folder `path`, a path the agent wrote, opened only when it lies below one of the
    /// folders the agent cannot replace, one name at a time without following a link.
    fn below_anchor(&self, path: &Path) -> Option<OwnedFd> {
        self.anchors.iter().find_map(|anchor| {
            let real = anchor.canonicalize().ok()?;
            let rest = path
                .strip_prefix(anchor)
                .or_else(|_| path.strip_prefix(&real))
                .ok()?;
            let mut dir = nix::fcntl::open(
                &real,
                OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .ok()?;
            for component in rest.components() {
                match component {
                    Component::Normal(name) => dir = open_dir_at(&dir, name, false).ok()?,
                    Component::CurDir => {}
                    _ => return None,
                }
            }
            Some(dir)
        })
    }

    fn note(&mut self, note: String) {
        if self.unread.len() < MAX_NOTES && !self.unread.contains(&note) {
            self.unread.push(note);
        }
    }
}

/// The folder `relative` (`./a/b`) below `root`, one name at a time without following a link;
/// `None` when it names `..`, is absolute or cannot be opened.
fn walk(root: &OwnedFd, relative: &str) -> Option<OwnedFd> {
    let mut dir = open_dir_at(root, OsStr::new("."), false).ok()?;
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(name) => dir = open_dir_at(&dir, name, false).ok()?,
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(dir)
}

/// Whether the JSON object `value` has `lspServers` with a value other than `null`.
fn has_servers(value: &Value) -> bool {
    value
        .get(LSP_SERVERS)
        .is_some_and(|servers| !servers.is_null())
}

/// Up to [`MAX_VERSIONS`] names in the folder `plugin`, `.` and `..` left out.
fn names_in(plugin: &OwnedFd) -> Vec<OsString> {
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

/// Whether `name` is one plain file name: not empty, not `.` or `..`, no `/` and no NUL.
fn plain(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

#[cfg(test)]
mod tests {
    use super::{LanguageServerPlugins, language_server_plugins};
    use serde_json::json;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const SERVER: &str =
        r#"{"x": {"command": "x-language-server", "extensionToLanguage": {".x": "x"}}}"#;

    /// A sandbox directory `<dir>` with the config dir `<dir>/home/.claude`, its plugin folder
    /// and the installed `ids`; the worktree is `<dir>/work`.
    fn config_with(dir: &Path, ids: &[&str]) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let config = dir.join("home").join(".claude");
        std::fs::create_dir_all(config.join("plugins"))?;
        std::fs::create_dir_all(dir.join("work"))?;
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

    /// The lookup for `config` (`<sandbox>/home/.claude`) and the worktree `<sandbox>/work`.
    fn lookup(config: &Path) -> LanguageServerPlugins {
        let sandbox = config
            .parent()
            .and_then(Path::parent)
            .unwrap_or(Path::new("/"));
        language_server_plugins(config, sandbox, &sandbox.join("work"))
    }

    fn found(config: &Path) -> Vec<String> {
        lookup(config).plugins
    }

    /// A marketplace folder `root` with the catalog entries `plugins` (and `extra` keys).
    fn catalog_at(
        root: &Path,
        plugins: &serde_json::Value,
        extra: serde_json::Value,
    ) -> TestResult {
        std::fs::create_dir_all(root.join(".claude-plugin"))?;
        let mut catalog = json!({ "name": "m", "plugins": plugins });
        if let (Some(catalog), Some(extra)) = (catalog.as_object_mut(), extra.as_object()) {
            catalog.extend(extra.clone());
        }
        std::fs::write(
            root.join(".claude-plugin/marketplace.json"),
            catalog.to_string(),
        )?;
        Ok(())
    }

    /// The catalog of the marketplace `name` where Claude Code clones one.
    fn catalog(config: &Path, name: &str, plugins: serde_json::Value) -> TestResult {
        catalog_at(
            &config.join("plugins/marketplaces").join(name),
            &plugins,
            json!({}),
        )
    }

    /// `known_marketplaces.json`: the marketplace `name` lies at `location`.
    fn known(config: &Path, name: &str, location: &Path) -> TestResult {
        std::fs::write(
            config.join("plugins/known_marketplaces.json"),
            json!({ name: {
                "source": { "source": "directory", "path": location },
                "installLocation": location,
            } })
            .to_string(),
        )?;
        Ok(())
    }

    /// The folder of version `version` of the plugin `id` in the cache, made.
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

    fn servers() -> serde_json::Value {
        serde_json::from_str(SERVER).unwrap_or_default()
    }

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
        catalog(
            &config,
            "official",
            json!([
                { "name": "skills-only", "source": "./plugins/skills-only" },
                { "name": "by-catalog", "source": "./plugins/by-catalog", "strict": false,
                  "lspServers": servers() },
                // Named in the catalog, neither installed nor switched on: not this session's.
                { "name": "not-installed", "lspServers": servers() },
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
        let all = lookup(&config);
        assert_eq!(
            all.plugins,
            ["by-catalog@official", "by-file@other", "by-manifest@other"]
        );
        assert!(all.unread.is_empty(), "{:?}", all.unread);
        Ok(())
    }

    /// An older version's folder that still lies in the cache counts too: which one Claude Code
    /// loads is its own choice.
    #[test]
    fn any_version_folder_of_a_plugin_counts() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config_with(dir.path(), &["p@m"])?;
        folder(&config, "p@m", "2.0.0")?;
        assert!(found(&config).is_empty());
        let old = folder(&config, "p@m", "1.0.0")?;
        std::fs::write(old.join(".lsp.json"), SERVER)?;
        assert_eq!(found(&config), ["p@m"]);
        Ok(())
    }

    /// Claude Code keeps a marketplace that is a folder where it lies and names the place in
    /// `known_marketplaces.json`; the installed copy of a plugin whose servers are only in the
    /// catalog holds nothing that says so (measured with `claude plugin marketplace add <dir>`
    /// and `claude plugin install` on 2.1.323). The catalog is read where it lies, when that is
    /// below the sandbox directory.
    #[test]
    fn a_marketplace_kept_in_a_folder_of_its_own_is_read_where_it_lies() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sandbox = dir.path().join("sandbox");
        let config = config_with(&sandbox, &["by-catalog@m", "skills@m"])?;
        let market = sandbox.join("work/tools/market");
        catalog_at(
            &market,
            &json!([{ "name": "by-catalog", "lspServers": servers() }, { "name": "skills" }]),
            json!({}),
        )?;
        let copy = folder(&config, "by-catalog@m", "1.0.0")?;
        std::fs::write(copy.join("README.md"), "Only this.")?;
        assert!(found(&config).is_empty(), "the place is not known yet");
        known(&config, "m", &market)?;
        assert_eq!(found(&config), ["by-catalog@m"]);

        // The same marketplace outside the sandbox directory, or behind a link, is not opened.
        let outside = dir.path().join("elsewhere/market");
        catalog_at(
            &outside,
            &json!([{ "name": "by-catalog", "lspServers": servers() }]),
            json!({}),
        )?;
        known(&config, "m", &outside)?;
        assert!(found(&config).is_empty(), "outside the sandbox directory");
        symlink(&outside, sandbox.join("work/linked"))?;
        known(&config, "m", &sandbox.join("work/linked"))?;
        assert!(found(&config).is_empty(), "through a link");
        known(&config, "m", &sandbox.join("work/../../elsewhere/market"))?;
        assert!(found(&config).is_empty(), "through `..`");
        Ok(())
    }

    /// A plugin a settings file switches on is loaded in place from a marketplace that is a
    /// folder, with no install record (measured on 2.1.323): the home's settings, the worktree's
    /// `.claude/settings.json` and `.claude/settings.local.json` name candidates too, and such a
    /// plugin's folder is the entry's `source` in the marketplace.
    #[test]
    fn a_plugin_switched_on_without_an_install_record_is_found_in_its_marketplace() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config_with(dir.path(), &[])?;
        let market = dir.path().join("work/market");
        catalog_at(
            &market,
            &json!([
                { "name": "by-catalog", "source": "./plugins/by-catalog", "lspServers": servers() },
                { "name": "by-file", "source": "./plugins/by-file" },
                { "name": "by-manifest", "source": "./plugins/by-manifest" },
                { "name": "bare", "source": "bare" },
                { "name": "climbing", "source": "./../../outside" },
                { "name": "skills", "source": "./plugins/skills" },
                { "name": "off", "source": "./plugins/by-file" },
            ]),
            json!({ "metadata": { "pluginRoot": "./rooted" } }),
        )?;
        known(&config, "m", &market)?;
        std::fs::create_dir_all(market.join("plugins/by-file"))?;
        std::fs::write(market.join("plugins/by-file/.lsp.json"), SERVER)?;
        manifest(
            &market.join("plugins/by-manifest"),
            &json!({ "name": "by-manifest", "lspServers": servers() }),
        )?;
        manifest(&market.join("plugins/skills"), &json!({ "name": "skills" }))?;
        std::fs::create_dir_all(market.join("rooted/bare"))?;
        std::fs::write(market.join("rooted/bare/.lsp.json"), SERVER)?;
        // What `./../../outside` would reach from the marketplace.
        std::fs::create_dir_all(dir.path().join("outside"))?;
        std::fs::write(dir.path().join("outside/.lsp.json"), SERVER)?;
        assert!(found(&config).is_empty(), "nothing is switched on yet");

        std::fs::write(
            config.join("settings.json"),
            r#"{"enabledPlugins": {"by-catalog@m": true, "skills@m": true, "off@m": false}}"#,
        )?;
        assert_eq!(found(&config), ["by-catalog@m"]);
        let project = dir.path().join("work/.claude");
        std::fs::create_dir_all(&project)?;
        std::fs::write(
            project.join("settings.json"),
            r#"{"enabledPlugins": {"by-file@m": true, "climbing@m": true}}"#,
        )?;
        std::fs::write(
            project.join("settings.local.json"),
            r#"{"enabledPlugins": {"by-manifest@m": true, "bare@m": true}}"#,
        )?;
        assert_eq!(
            found(&config),
            ["bare@m", "by-catalog@m", "by-file@m", "by-manifest@m"]
        );
        Ok(())
    }

    /// An install record names where the copy lies; a copy outside the cache is looked at when
    /// it lies below the sandbox directory, and nowhere else.
    #[test]
    fn an_installed_copy_outside_the_cache_is_looked_at_below_the_sandbox_directory() -> TestResult
    {
        let dir = tempfile::tempdir()?;
        let sandbox = dir.path().join("sandbox");
        let config = config_with(&sandbox, &[])?;
        let record = |path: &Path| -> std::io::Result<()> {
            std::fs::write(
                config.join("plugins/installed_plugins.json"),
                json!({ "version": 2, "plugins": { "p@m": [{ "installPath": path }] } })
                    .to_string(),
            )
        };
        let inside = sandbox.join("work/copies/p");
        let outside = dir.path().join("copies/p");
        for copy in [&inside, &outside] {
            std::fs::create_dir_all(copy)?;
            std::fs::write(copy.join(".lsp.json"), SERVER)?;
        }
        record(&outside)?;
        assert!(found(&config).is_empty());
        record(&inside)?;
        assert_eq!(found(&config), ["p@m"]);
        Ok(())
    }

    /// Nothing is created or written, and a home without Claude Code's folders has no plugins.
    #[test]
    fn a_missing_config_dir_has_no_plugins_and_stays_missing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join("home").join(".claude");
        assert_eq!(lookup(&config), LanguageServerPlugins::default());
        assert!(!dir.path().join("home").exists(), "nothing is created");
        std::fs::create_dir_all(&config)?;
        assert_eq!(lookup(&config), LanguageServerPlugins::default());
        assert_eq!(std::fs::read_dir(&config)?.count(), 0);
        // Files that are not what Claude Code writes bring nothing either.
        std::fs::create_dir_all(config.join("plugins"))?;
        for text in ["", "not json", "[]", r#"{"plugins": []}"#] {
            std::fs::write(config.join("plugins/installed_plugins.json"), text)?;
            assert!(found(&config).is_empty(), "{text:?}");
        }
        Ok(())
    }

    /// ADR-0174: the config dir is the agent's. A link on the way to a file, or in its place, is
    /// never followed (a catalog or a plugin folder outside the home stays unread), a FIFO holds
    /// nothing, and what was there and not read is said.
    #[test]
    fn the_lookup_follows_no_link_and_waits_on_no_fifo() -> TestResult {
        let dir = tempfile::tempdir()?;
        // What a link could reach: a full plugin folder and a catalog outside the home.
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(outside.join(".claude-plugin"))?;
        std::fs::write(outside.join(".lsp.json"), SERVER)?;
        let entries = json!({ "name": "m", "plugins": [{ "name": "p", "lspServers": servers() }] });
        std::fs::write(
            outside.join(".claude-plugin/marketplace.json"),
            entries.to_string(),
        )?;
        std::fs::write(outside.join("servers.json"), SERVER)?;
        std::fs::write(
            outside.join(".claude-plugin/plugin.json"),
            json!({ "name": "p", "lspServers": servers() }).to_string(),
        )?;

        // The version folder is a link.
        let config = config_with(&dir.path().join("a"), &["p@m"])?;
        std::fs::create_dir_all(config.join("plugins/cache/m/p"))?;
        symlink(&outside, config.join("plugins/cache/m/p/1.0.0"))?;
        assert!(found(&config).is_empty());
        // `.lsp.json` is a link, then a FIFO; the manifest a link, then a FIFO.
        let config = config_with(&dir.path().join("b"), &["p@m"])?;
        let folder_b = folder(&config, "p@m", "1.0.0")?;
        symlink(outside.join("servers.json"), folder_b.join(".lsp.json"))?;
        assert!(found(&config).is_empty());
        std::fs::remove_file(folder_b.join(".lsp.json"))?;
        nix::unistd::mkfifo(&folder_b.join(".lsp.json"), nix::sys::stat::Mode::S_IRWXU)?;
        assert!(found(&config).is_empty());
        std::fs::create_dir_all(folder_b.join(".claude-plugin"))?;
        symlink(
            outside.join(".claude-plugin/plugin.json"),
            folder_b.join(".claude-plugin/plugin.json"),
        )?;
        let linked = lookup(&config);
        assert!(linked.plugins.is_empty());
        assert_eq!(linked.unread, ["plugin.json is a link"]);
        std::fs::remove_file(folder_b.join(".claude-plugin/plugin.json"))?;
        nix::unistd::mkfifo(
            &folder_b.join(".claude-plugin/plugin.json"),
            nix::sys::stat::Mode::S_IRWXU,
        )?;
        let fifo = lookup(&config);
        assert!(fifo.plugins.is_empty());
        assert_eq!(fifo.unread, ["plugin.json is not a regular file"]);
        // The marketplace's folder is a link, then its catalog a FIFO.
        let config = config_with(&dir.path().join("c"), &["p@m"])?;
        std::fs::create_dir_all(config.join("plugins/marketplaces"))?;
        symlink(&outside, config.join("plugins/marketplaces/m"))?;
        assert!(found(&config).is_empty());
        std::fs::remove_file(config.join("plugins/marketplaces/m"))?;
        std::fs::create_dir_all(config.join("plugins/marketplaces/m/.claude-plugin"))?;
        nix::unistd::mkfifo(
            &config.join("plugins/marketplaces/m/.claude-plugin/marketplace.json"),
            nix::sys::stat::Mode::S_IRWXU,
        )?;
        assert!(found(&config).is_empty());
        // The plugin folder, the list of installed plugins, the config dir and the home as links.
        let real = config_with(&dir.path().join("d"), &["p@m"])?;
        catalog(
            &real,
            "m",
            json!([{ "name": "p", "lspServers": servers() }]),
        )?;
        assert_eq!(found(&real), ["p@m"], "the control");
        let linked = dir.path().join("e/home/.claude");
        std::fs::create_dir_all(&linked)?;
        symlink(real.join("plugins"), linked.join("plugins"))?;
        assert!(found(&linked).is_empty());
        std::fs::remove_file(linked.join("plugins"))?;
        std::fs::create_dir_all(linked.join("plugins"))?;
        symlink(
            real.join("plugins/installed_plugins.json"),
            linked.join("plugins/installed_plugins.json"),
        )?;
        assert!(found(&linked).is_empty());
        std::fs::create_dir_all(dir.path().join("f/home"))?;
        symlink(&real, dir.path().join("f/home/.claude"))?;
        assert!(found(&dir.path().join("f/home/.claude")).is_empty());
        std::fs::create_dir_all(dir.path().join("g"))?;
        symlink(dir.path().join("d/home"), dir.path().join("g/home"))?;
        assert!(found(&dir.path().join("g/home/.claude")).is_empty());
        // A worktree's `.claude` that is a link to a folder with settings names no candidate.
        let config = config_with(&dir.path().join("h"), &[])?;
        catalog(
            &config,
            "m",
            json!([{ "name": "p", "lspServers": servers() }]),
        )?;
        let settings = dir.path().join("h/elsewhere");
        std::fs::create_dir_all(&settings)?;
        std::fs::write(
            settings.join("settings.json"),
            r#"{"enabledPlugins": {"p@m": true}}"#,
        )?;
        symlink(&settings, dir.path().join("h/work/.claude"))?;
        assert!(found(&config).is_empty());
        std::fs::remove_file(dir.path().join("h/work/.claude"))?;
        std::fs::rename(&settings, dir.path().join("h/work/.claude"))?;
        assert_eq!(found(&config), ["p@m"], "the control");
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
        assert_eq!(found(&config), ["plain@m"]);
        Ok(())
    }

    /// The bounds of one file, one plugin and one id: a file larger than 2 MiB is not read, the
    /// first 16 version folders and 256 plugins are looked at, an id longer than 200 bytes is
    /// left out (a run's settings stay far below what a command line takes); each is said.
    #[test]
    fn the_lookup_is_bounded() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config_with(dir.path(), &["p@m"])?;
        let copy = folder(&config, "p@m", "1.0.0")?;
        manifest(&copy, &json!({ "name": "p", "lspServers": servers() }))?;
        assert_eq!(found(&config), ["p@m"], "the control");
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(copy.join(".claude-plugin/plugin.json"))?;
        file.set_len(2 * 1024 * 1024 + 1)?;
        let oversized = lookup(&config);
        assert!(oversized.plugins.is_empty());
        assert_eq!(
            oversized.unread,
            ["plugin.json is too large (2097153 bytes) or past the lookup's budget"]
        );

        // 300 installed plugins, each with a language server: the first 256 by id.
        let ids: Vec<String> = (0..300).map(|n| format!("p{n:03}@m")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let config = config_with(&dir.path().join("many"), &refs)?;
        let entries: Vec<serde_json::Value> = (0..300)
            .map(|n| json!({ "name": format!("p{n:03}"), "lspServers": {} }))
            .collect();
        catalog(&config, "m", json!(entries))?;
        let many = lookup(&config);
        assert_eq!(many.plugins, ids[..256]);
        assert_eq!(many.unread, ["44 plugins beyond the first 256"]);

        // 40 version folders, none with a server: 16 are opened.
        let config = config_with(&dir.path().join("versions"), &["p@m"])?;
        for n in 0..40 {
            folder(&config, "p@m", &format!("{n}.0.0"))?;
        }
        assert!(found(&config).is_empty());
        let versions: std::os::fd::OwnedFd =
            std::fs::File::open(config.join("plugins/cache/m/p"))?.into();
        assert_eq!(super::names_in(&versions).len(), 16);

        // An id of 201 bytes is left out though its catalog entry names a server.
        let long = format!("{}@m", "n".repeat(199));
        let config = config_with(&dir.path().join("long"), &[long.as_str(), "short@m"])?;
        catalog(
            &config,
            "m",
            json!([
                { "name": "n".repeat(199), "lspServers": {} },
                { "name": "short", "lspServers": {} },
            ]),
        )?;
        let ids = lookup(&config);
        assert_eq!(ids.plugins, ["short@m"]);
        assert_eq!(ids.unread, ["1 plugin ids longer than 200 bytes"]);
        Ok(())
    }

    /// The bound of the whole lookup: whatever the agent lays out, at most 16 MiB are read and
    /// 1024 plugin folders looked into, so a launch waits a moment, not minutes. Here 256 plugins
    /// of 256 marketplaces with a catalog of 2 MiB each and 16 manifests of 2 MiB each (all hard
    /// links to two files): unbounded, 256 x 17 files of 2 MiB.
    #[test]
    fn the_whole_lookup_reads_a_bounded_amount() -> TestResult {
        let dir = tempfile::tempdir()?;
        let ids: Vec<String> = (0..256).map(|n| format!("p@m{n:03}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let config = config_with(dir.path(), &refs)?;
        // Valid JSON of just under 2 MiB that names no language server.
        let filler = format!(
            r#"{{"plugins": [], "pad": "{}"}}"#,
            "x".repeat(2 * 1024 * 1024 - 64)
        );
        let big_catalog = dir.path().join("catalog.json");
        let big_manifest = dir.path().join("manifest.json");
        std::fs::write(&big_catalog, &filler)?;
        std::fs::write(&big_manifest, &filler)?;
        for n in 0..256 {
            let market = config.join(format!("plugins/marketplaces/m{n:03}/.claude-plugin"));
            std::fs::create_dir_all(&market)?;
            std::fs::hard_link(&big_catalog, market.join("marketplace.json"))?;
            for version in 0..16 {
                let copy = folder(&config, &format!("p@m{n:03}"), &format!("{version}.0.0"))?;
                std::fs::create_dir_all(copy.join(".claude-plugin"))?;
                std::fs::hard_link(&big_manifest, copy.join(".claude-plugin/plugin.json"))?;
            }
        }
        // The last plugin has a server, far past both budgets: it is not reached, and that is said.
        std::fs::write(config.join("plugins/cache/m255/p/0.0.0/.lsp.json"), SERVER)?;
        let mut lookup = super::Lookup::new(&config, dir.path());
        let plugins = lookup.find(&config, &dir.path().join("work"));
        assert!(plugins.is_empty(), "{plugins:?}");
        let read = super::MAX_TOTAL_BYTES - lookup.bytes_left;
        assert!(
            read >= super::MAX_TOTAL_BYTES - super::MAX_FILE_BYTES,
            "the budget of bytes is used and not passed: {read}"
        );
        assert_eq!(lookup.folders_left, 0, "the budget of folders is used");
        for note in [
            "past the lookup's budget",
            "plugin folders beyond the first 1024",
        ] {
            assert!(
                lookup.unread.iter().any(|said| said.ends_with(note)),
                "{note}: {:?}",
                lookup.unread
            );
        }
        // Within the budgets the same plugin is found.
        std::fs::write(config.join("plugins/cache/m000/p/0.0.0/.lsp.json"), SERVER)?;
        assert_eq!(found(&config), ["p@m000"]);
        Ok(())
    }
}
