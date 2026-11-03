//! CLI token persistence in `~/.config/yulius/credentials.json` (owner-only, one token per
//! server URL; tokens are never printed).

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::secret::Secret;

/// Current credentials file format version.
const CREDENTIALS_VERSION: u32 = 1;
/// Credentials files larger than this are refused.
const MAX_CREDENTIALS_BYTES: u64 = 1024 * 1024;

/// A credentials file problem. Messages never include token values.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    /// Neither `XDG_CONFIG_HOME` nor `HOME` is usable.
    #[error("cannot locate the configuration directory: set HOME")]
    NoHome,
    /// Reading or writing the file failed.
    #[error("credentials file {}: {source}", path.display())]
    Io {
        /// File or directory involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// The file is not a valid credentials file.
    #[error("credentials file {} is invalid; remove it and log in again", .0.display())]
    Corrupt(PathBuf),
}

#[derive(Serialize, Deserialize, Default)]
struct CredentialsFile {
    version: u32,
    servers: BTreeMap<String, StoredCredential>,
}

#[derive(Serialize, Deserialize)]
struct StoredCredential {
    /// A `Secret` from parsing to drop, so no plain copy outlives it (SPEC-407 TKS-002, #769).
    #[serde(with = "stored_secret")]
    token: Secret,
}

/// `Secret` has no `Serialize`, so it cannot reach output by accident; the credentials file is the
/// one place it is written, as the plain string it holds.
mod stored_secret {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::secret::Secret;

    pub(super) fn serialize<S: Serializer>(
        token: &Secret,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(token.expose())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Secret, D::Error> {
        String::deserialize(deserializer).map(Secret::new)
    }
}

/// Stores CLI bearer tokens keyed by normalized server URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    /// A store backed by the file at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The store at `$XDG_CONFIG_HOME/yulius/credentials.json` when `XDG_CONFIG_HOME` is an
    /// absolute path, otherwise `~/.config/yulius/credentials.json`.
    ///
    /// # Errors
    /// [`TokenStoreError::NoHome`] when no configuration directory can be determined.
    pub fn default_location() -> Result<Self, TokenStoreError> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".config"))
            })
            .ok_or(TokenStoreError::NoHome)?;
        Ok(Self::new(config.join("yulius").join("credentials.json")))
    }

    /// The store of remote-control tokens (`yulius remote login`, SPEC-247) beside this one:
    /// kept apart from `yulius login`'s, so neither replaces the other.
    #[must_use]
    pub fn remote(&self) -> Self {
        Self::new(self.path.with_file_name("remote-credentials.json"))
    }

    /// The backing file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Normalized key for `server`: `scheme://host[:port][/path]` without a trailing `/`,
    /// query or fragment.
    #[must_use]
    pub fn server_key(server: &Url) -> String {
        let mut key = format!(
            "{}://{}",
            server.scheme(),
            server.host_str().unwrap_or_default()
        );
        if let Some(port) = server.port() {
            key.push_str(&format!(":{port}"));
        }
        key.push_str(server.path().trim_end_matches('/'));
        key
    }

    /// The token saved for `server`, if any.
    ///
    /// # Errors
    /// I/O failure or a corrupt file.
    pub fn load(&self, server: &Url) -> Result<Option<Secret>, TokenStoreError> {
        Ok(self
            .read()?
            .servers
            .remove(&Self::server_key(server))
            .map(|stored| stored.token))
    }

    /// Saves `token` for `server`, replacing any previous one. The file is written atomically
    /// with owner-only permissions (directory `0700`, file `0600` on Unix).
    ///
    /// # Errors
    /// I/O failure or a corrupt existing file.
    pub fn save(&self, server: &Url, token: &Secret) -> Result<(), TokenStoreError> {
        let mut file = self.read()?;
        file.servers.insert(
            Self::server_key(server),
            StoredCredential {
                token: token.clone(),
            },
        );
        self.write(&file)
    }

    /// The servers with a saved token (normalized keys, sorted), so `yulius onboard` can use the
    /// only one without `--server`.
    ///
    /// # Errors
    /// I/O failure or a corrupt file.
    pub fn servers(&self) -> Result<Vec<String>, TokenStoreError> {
        Ok(self.read()?.servers.into_keys().collect())
    }

    /// The token saved here for `server`, else the one `yulius remote login` saved for it
    /// (SPEC-256 LOB-001): a person's agent signs in once, with the token made for it.
    ///
    /// # Errors
    /// I/O failure or a corrupt file.
    pub fn load_or_remote(&self, server: &Url) -> Result<Option<Secret>, TokenStoreError> {
        match self.load(server)? {
            Some(token) => Ok(Some(token)),
            None => self.remote().load(server),
        }
    }

    /// The servers with a token here or in the remote store, each once, sorted.
    ///
    /// # Errors
    /// I/O failure or a corrupt file.
    pub fn known_servers(&self) -> Result<Vec<String>, TokenStoreError> {
        let mut servers = self.servers()?;
        servers.extend(self.remote().servers()?);
        servers.sort();
        servers.dedup();
        Ok(servers)
    }

    /// Forgets the token for `server`; returns whether one existed.
    ///
    /// # Errors
    /// I/O failure or a corrupt file.
    pub fn remove(&self, server: &Url) -> Result<bool, TokenStoreError> {
        let mut file = self.read()?;
        let existed = file.servers.remove(&Self::server_key(server)).is_some();
        if existed {
            self.write(&file)?;
        }
        Ok(existed)
    }

    fn io_error(&self, path: &Path) -> impl FnOnce(io::Error) -> TokenStoreError {
        let path = path.to_path_buf();
        move |source| TokenStoreError::Io { path, source }
    }

    /// Reads the file through one handle: a regular file only, at most
    /// `MAX_CREDENTIALS_BYTES` read from it, whatever its length was when opened (SPEC-407
    /// TKS-001, #770).
    fn read(&self) -> Result<CredentialsFile, TokenStoreError> {
        let file = match open_for_read(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CredentialsFile {
                    version: CREDENTIALS_VERSION,
                    servers: BTreeMap::new(),
                });
            }
            Err(error) => return Err(self.io_error(&self.path)(error)),
        };
        let meta = file.metadata().map_err(self.io_error(&self.path))?;
        if !meta.is_file() || meta.len() > MAX_CREDENTIALS_BYTES {
            return Err(TokenStoreError::Corrupt(self.path.clone()));
        }
        let bytes = read_bounded(file)
            .map_err(self.io_error(&self.path))?
            .ok_or_else(|| TokenStoreError::Corrupt(self.path.clone()))?;
        let file: CredentialsFile = serde_json::from_slice(&bytes)
            .map_err(|_| TokenStoreError::Corrupt(self.path.clone()))?;
        if file.version != CREDENTIALS_VERSION {
            return Err(TokenStoreError::Corrupt(self.path.clone()));
        }
        Ok(file)
    }

    fn write(&self, file: &CredentialsFile) -> Result<(), TokenStoreError> {
        let dir = self
            .path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        create_private_dir(dir).map_err(self.io_error(dir))?;
        // A fresh random name, created with O_EXCL and 0600: nothing left at a fixed name blocks
        // the save or is written through (SPEC-407 TKS-003, #771). Dropped on a failure, the
        // temporary file is removed, so no token copy stays behind.
        let mut temp = private_temp_file(dir).map_err(self.io_error(dir))?;
        write_private_file(temp.as_file_mut(), file).map_err(self.io_error(temp.path()))?;
        temp.persist(&self.path)
            .map_err(|error| self.io_error(&self.path)(error.error))?;
        set_private_permissions(&self.path).map_err(self.io_error(&self.path))?;
        if let Ok(directory) = File::open(dir) {
            // Directory fsync makes the rename durable where supported.
            let _ = directory.sync_all();
        }
        Ok(())
    }
}

/// At most `MAX_CREDENTIALS_BYTES` of `source`, `None` when it holds more. The buffer is sized once
/// for the bound, so it never grows and leaves no unzeroed copy, and is zeroed when dropped.
fn read_bounded(source: impl Read) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(
        usize::try_from(MAX_CREDENTIALS_BYTES + 1).unwrap_or(usize::MAX),
    ));
    source
        .take(MAX_CREDENTIALS_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= MAX_CREDENTIALS_BYTES).then_some(bytes))
}

/// Opens `path` to read without waiting: a FIFO with no writer would block the open.
#[cfg(unix)]
fn open_for_read(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
        .open(path)
}

#[cfg(not(unix))]
fn open_for_read(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

/// An owner-only (`0600`) file with a random name in `dir`, created with `O_EXCL`.
fn private_temp_file(dir: &Path) -> io::Result<tempfile::NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".credentials.json.tmp-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o600));
    }
    builder.tempfile_in(dir)
}

/// Writes `contents` to `file` unbuffered and syncs it: the tokens go to the file only, never to a
/// growing buffer that would free unzeroed copies (SPEC-407 TKS-002, #769).
fn write_private_file(file: &mut File, contents: &CredentialsFile) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *file, contents).map_err(io::Error::other)?;
    file.sync_all()
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn saves_and_loads_per_server() -> TestResult {
        let dir = tempfile::tempdir()?;
        let store = TokenStore::new(dir.path().join("nested").join("credentials.json"));
        let prod = Url::parse("https://yulius.quartio.ai/")?;
        let local = Url::parse("http://127.0.0.1:8080")?;
        assert_eq!(store.load(&prod)?, None);
        store.save(&prod, &Secret::new("yul_prod"))?;
        store.save(&local, &Secret::new("yul_local"))?;
        store.save(&prod, &Secret::new("yul_prod_2"))?;
        assert_eq!(
            store.load(&Url::parse("https://yulius.quartio.ai")?)?,
            Some(Secret::new("yul_prod_2"))
        );
        assert_eq!(store.load(&local)?, Some(Secret::new("yul_local")));
        assert!(store.remove(&local)?);
        assert!(!store.remove(&local)?);
        assert_eq!(store.load(&local)?, None);
        assert_eq!(store.load(&prod)?, Some(Secret::new("yul_prod_2")));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn file_and_directory_are_owner_only() -> TestResult {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("yulius").join("credentials.json");
        let store = TokenStore::new(path.clone());
        store.save(&Url::parse("https://y.example")?, &Secret::new("yul_x"))?;
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(dir.path().join("yulius"))?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("yulius"))?
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty());
        Ok(())
    }

    #[test]
    fn corrupt_files_are_reported_without_contents() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("credentials.json");
        fs::write(&path, b"{\"token\": \"yul_leak\"")?;
        let store = TokenStore::new(path);
        let error = store.load(&Url::parse("https://y.example")?);
        match error {
            Err(TokenStoreError::Corrupt(_)) => {}
            other => return Err(format!("expected Corrupt, got {other:?}").into()),
        }
        assert!(
            store
                .save(&Url::parse("https://y.example")?, &Secret::new("t"))
                .is_err()
        );
        Ok(())
    }

    /// SPEC-407 TKS-003 (#771): the temporary file has a fresh random name, so nothing left at a
    /// predictable name (here a folder, which no removal clears) blocks a save or is written
    /// through, and no temporary file of the save remains.
    #[test]
    fn a_squatted_temporary_name_does_not_block_a_save() -> TestResult {
        let dir = tempfile::tempdir()?;
        let squatted = dir
            .path()
            .join(format!(".credentials.json.tmp-{}", std::process::id()));
        fs::create_dir(&squatted)?;
        let store = TokenStore::new(dir.path().join("credentials.json"));
        let server = Url::parse("https://y.example")?;
        store.save(&server, &Secret::new("yul_x"))?;
        assert_eq!(store.load(&server)?, Some(Secret::new("yul_x")));
        assert!(fs::read_dir(&squatted)?.next().is_none());
        let mut names: Vec<_> = fs::read_dir(dir.path())?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()?;
        names.sort();
        assert_eq!(
            names,
            [
                squatted.file_name().ok_or("name")?,
                "credentials.json".as_ref()
            ]
        );
        Ok(())
    }

    /// SPEC-407 TKS-002 (#769): a parsed token is a `Secret` (wiped when dropped) from parsing on,
    /// every server's, and it is written back as the plain string the file holds.
    #[test]
    fn stored_tokens_are_secrets() -> TestResult {
        let text = r#"{"version":1,"servers":{"https://a.example":{"token":"yul_a"},"https://b.example":{"token":"yul_b"}}}"#;
        let file: CredentialsFile = serde_json::from_str(text)?;
        let tokens: Vec<&Secret> = file.servers.values().map(|stored| &stored.token).collect();
        assert_eq!(tokens, [&Secret::new("yul_a"), &Secret::new("yul_b")]);
        assert_eq!(serde_json::to_string(&file)?, text);
        Ok(())
    }

    /// SPEC-407 TKS-001 (#770): what is read is bounded too, not only the length looked up when
    /// the file was opened (a file that grows in between): the bound itself is read, one more
    /// byte is refused.
    #[test]
    fn reads_stop_at_the_bound() -> TestResult {
        let bound = usize::try_from(MAX_CREDENTIALS_BYTES)?;
        let at_bound = read_bounded(io::repeat(b' ').take(MAX_CREDENTIALS_BYTES))?;
        assert_eq!(at_bound.map(|bytes| bytes.len()), Some(bound));
        assert!(read_bounded(io::repeat(b' ').take(MAX_CREDENTIALS_BYTES + 1))?.is_none());
        assert!(
            read_bounded(io::repeat(b' '))?.is_none(),
            "an endless source"
        );
        Ok(())
    }

    /// SPEC-407 TKS-001 (#770): the size bound holds for what is read, not for a length looked up
    /// before: a FIFO (length 0) fed more than 1 MiB of valid JSON is refused, without waiting
    /// for a writer, and so is a regular file over the bound.
    #[cfg(unix)]
    #[test]
    fn an_oversized_or_special_credentials_file_is_refused() -> TestResult {
        use std::io::{Read as _, Write as _};
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("credentials.json");
        let server = Url::parse("https://y.example")?;
        let padded = |len: usize| {
            let mut json = vec![b' '; len];
            json.extend_from_slice(br#"{"version":1,"servers":{}}"#);
            json
        };
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRWXU)?;
        // Read and write: the open neither waits for a reader nor ends the stream for one.
        let mut fifo = OpenOptions::new().read(true).write(true).open(&path)?;
        let json = padded(usize::try_from(MAX_CREDENTIALS_BYTES)? + 1);
        let writer = std::thread::spawn(move || {
            // Refused or read whole, the stream ends when this end closes.
            let _ = fifo.write_all(&json);
        });
        match TokenStore::new(path.clone()).load(&server) {
            Err(TokenStoreError::Corrupt(_)) => {}
            other => return Err(format!("a FIFO: expected Corrupt, got {other:?}").into()),
        }
        // Drain what the refused read left, so the writer ends.
        let mut rest = Vec::new();
        File::open(&path)?.read_to_end(&mut rest)?;
        writer.join().map_err(|_| "writer panicked")?;
        // No writer at all: refused at once.
        assert!(matches!(
            TokenStore::new(path.clone()).load(&server),
            Err(TokenStoreError::Corrupt(_))
        ));
        fs::remove_file(&path)?;
        fs::write(&path, padded(usize::try_from(MAX_CREDENTIALS_BYTES)?))?;
        assert!(matches!(
            TokenStore::new(path).load(&server),
            Err(TokenStoreError::Corrupt(_))
        ));
        Ok(())
    }

    /// SPEC-256 LOB-001: `yulius login`'s token first, else the one `yulius remote login` stored
    /// for the same instance; the servers of both are known.
    #[test]
    fn the_remote_token_stands_in_for_a_missing_login_token() -> TestResult {
        let dir = tempfile::tempdir()?;
        let store = TokenStore::new(dir.path().join("credentials.json"));
        let laptop = Url::parse("https://y.example/")?;
        let other = Url::parse("https://other.example")?;
        assert_eq!(store.load_or_remote(&laptop)?, None);
        assert!(store.known_servers()?.is_empty());
        store.remote().save(&laptop, &Secret::new("yul_operate"))?;
        assert_eq!(
            store.load_or_remote(&laptop)?,
            Some(Secret::new("yul_operate"))
        );
        assert_eq!(store.load(&laptop)?, None, "the login store is untouched");
        assert_eq!(store.known_servers()?, vec!["https://y.example".to_owned()]);
        store.save(&laptop, &Secret::new("yul_cli"))?;
        store.save(&other, &Secret::new("yul_other"))?;
        assert_eq!(store.load_or_remote(&laptop)?, Some(Secret::new("yul_cli")));
        assert_eq!(
            store.known_servers()?,
            vec![
                "https://other.example".to_owned(),
                "https://y.example".to_owned()
            ],
            "once each"
        );
        Ok(())
    }

    #[test]
    fn server_keys_normalize_trailing_slash_query_and_default_port() -> TestResult {
        assert_eq!(
            TokenStore::server_key(&Url::parse("https://Y.example:443/base/?x=1#f")?),
            "https://y.example/base"
        );
        assert_eq!(
            TokenStore::server_key(&Url::parse("http://localhost:8080/")?),
            "http://localhost:8080"
        );
        Ok(())
    }
}
