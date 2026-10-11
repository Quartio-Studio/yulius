//! CLI token persistence in `~/.config/yulius/credentials.json` (owner-only, one token per
//! server URL; tokens are never printed).

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use reqwest::Url;
use serde::{Deserialize, Serialize};

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
    token: String,
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
            .map(|stored| Secret::new(stored.token)))
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
                token: token.expose().to_owned(),
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

    fn read(&self) -> Result<CredentialsFile, TokenStoreError> {
        let bytes = match fs::metadata(&self.path) {
            Ok(meta) if meta.len() > MAX_CREDENTIALS_BYTES => {
                return Err(TokenStoreError::Corrupt(self.path.clone()));
            }
            Ok(_) => fs::read(&self.path).map_err(self.io_error(&self.path))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CredentialsFile {
                    version: CREDENTIALS_VERSION,
                    servers: BTreeMap::new(),
                });
            }
            Err(error) => return Err(self.io_error(&self.path)(error)),
        };
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
        let json = serde_json::to_vec_pretty(file)
            .map_err(|error| self.io_error(&self.path)(io::Error::other(error)))?;
        let temp = dir.join(format!(".credentials.json.tmp-{}", std::process::id()));
        // A stale temporary file from a crashed run is replaced.
        match fs::remove_file(&temp) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(self.io_error(&temp)(error)),
        }
        let result = write_private_file(&temp, &json).and_then(|()| fs::rename(&temp, &self.path));
        if let Err(error) = result {
            // Best effort: do not leave a token copy behind.
            let _ = fs::remove_file(&temp);
            return Err(self.io_error(&self.path)(error));
        }
        set_private_permissions(&self.path).map_err(self.io_error(&self.path))?;
        if let Ok(directory) = File::open(dir) {
            // Directory fsync makes the rename durable where supported.
            let _ = directory.sync_all();
        }
        Ok(())
    }
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

fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
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
