//! Stores copied from a migrated template: a test copies one file (about 6 ms) instead of
//! running every migration (about 200 ms).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use yulius_store::SqliteStore;

type BoxError = Box<dyn std::error::Error>;

/// Opens the store at `path`, first copying the migrated template there when no file exists.
/// An existing file (a restarted server's, or an older schema a test wrote) opens as it is.
pub fn migrated(path: &Path) -> Result<SqliteStore, BoxError> {
    if !path.exists() {
        std::fs::copy(template()?, path)?;
    }
    Ok(SqliteStore::open(path)?)
}

/// The template this test binary migrated, shared by its test processes: one file per schema
/// version and build of the binary, written once under a temporary name and renamed into place.
fn template() -> Result<&'static Path, BoxError> {
    static TEMPLATE: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| write_template().map_err(|err| err.to_string()))
        .as_deref()
        .map_err(|err| err.clone().into())
}

fn write_template() -> Result<PathBuf, BoxError> {
    let exe = std::fs::metadata(std::env::current_exe()?)?;
    let built = exe
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let name = format!(
        "store-template-v{}-{built}-{}.db",
        yulius_store::schema_version(),
        exe.len()
    );
    let path = dir.join(name);
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(dir)?;
    let partial = path.with_extension(format!("{}.partial", std::process::id()));
    std::fs::File::create(&partial)?;
    yulius_store::migrate_file(&partial)?;
    std::fs::rename(&partial, &path)?;
    Ok(path)
}
