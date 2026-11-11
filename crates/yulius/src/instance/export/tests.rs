use std::fs;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
use crate::instance::archive::{Item, Reader};

fn tree() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fs::create_dir_all(root.join("sandboxes/sb/home/.claude"))?;
    fs::create_dir_all(root.join("system/base-1"))?;
    fs::write(root.join("yulius.db"), vec![7u8; 70_000])?;
    fs::write(root.join("master.key"), b"k")?;
    fs::write(root.join("sandboxes/sb/home/.claude/s.jsonl"), b"{}\n")?;
    fs::write(root.join("system/base-1/big"), vec![1u8; 10_000])?;
    Ok(dir)
}

fn manifest() -> Manifest {
    Manifest {
        format: crate::instance::FORMAT,
        data_dir: "/var/lib/yulius".into(),
        ..Manifest::default()
    }
}

#[test]
fn the_stream_is_exactly_the_announced_length_and_reads_back() -> TestResult {
    let dir = tree()?;
    let plan = plan(dir.path())?;
    let mut out = Vec::new();
    let mut ticks = Vec::new();
    let sent = send(
        dir.path(),
        &plan,
        &manifest(),
        &mut out,
        &mut |d| ticks.push(d),
        &mut || Ok(()),
    )?;
    assert_eq!(sent, plan.total);
    assert_eq!(out.len() as u64, plan.total);
    assert!(ticks.windows(2).all(|w| w[0] <= w[1]) && !ticks.is_empty());

    let mut r = Reader::new(out.as_slice());
    let Item::Entry(first) = r.next_item()? else {
        panic!("no manifest")
    };
    assert_eq!(first.path, MANIFEST);
    let mut json = vec![0u8; MANIFEST_LEN as usize];
    let mut at = 0;
    while at < json.len() {
        at += r.data(&mut json[at..])?;
    }
    let m: Manifest = serde_json::from_slice(&json)?;
    assert_eq!(m.total_bytes, plan.total);
    assert_eq!(m.entries, plan.entries);
    let mut paths = Vec::new();
    while let Item::Entry(e) = r.next_item()? {
        paths.push(String::from_utf8(e.path)?);
    }
    assert_eq!(paths.len() as u64, plan.entries);
    assert!(paths.contains(&"data/sandboxes/sb/home/.claude/s.jsonl".to_owned()));
    assert!(
        !paths.iter().any(|p| p.starts_with("data/system")),
        "{paths:?}"
    );
    Ok(())
}

#[test]
fn a_file_that_changed_after_the_plan_aborts_the_stream() -> TestResult {
    let dir = tree()?;
    let plan = plan(dir.path())?;
    fs::write(dir.path().join("master.key"), b"longer key")?;
    let err = send(
        dir.path(),
        &plan,
        &manifest(),
        Vec::new(),
        &mut |_| {},
        &mut || Ok(()),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(err.to_string().contains("during the export"), "{err}");
    Ok(())
}

#[test]
fn a_new_file_after_the_plan_aborts_the_stream() -> TestResult {
    let dir = tree()?;
    let plan = plan(dir.path())?;
    fs::write(dir.path().join("zzz"), vec![0u8; 5000])?;
    let err = send(
        dir.path(),
        &plan,
        &manifest(),
        Vec::new(),
        &mut |_| {},
        &mut || Ok(()),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(
        err.to_string().contains("changed during the export"),
        "{err}"
    );
    Ok(())
}

#[test]
fn a_reader_that_goes_away_fails_the_stream() -> TestResult {
    struct Gone;
    impl Write for Gone {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let dir = tree()?;
    let p = plan(dir.path())?;
    let err = send(dir.path(), &p, &manifest(), Gone, &mut |_| {}, &mut || {
        Ok(())
    })
    .err()
    .ok_or("expected an error")?;
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    Ok(())
}

#[test]
fn an_update_found_before_the_end_leaves_the_copy_without_its_checksum() -> TestResult {
    let dir = tree()?;
    let p = plan(dir.path())?;
    let mut out = Vec::new();
    let err = send(
        dir.path(),
        &p,
        &manifest(),
        &mut out,
        &mut |_| {},
        &mut || Err(io::Error::other("an update is running")),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(err.to_string().contains("an update is running"), "{err}");
    let mut r = Reader::new(out.as_slice());
    let ended = loop {
        match r.next_item() {
            Ok(Item::Entry(_)) => {}
            Ok(Item::End) => break None,
            Err(e) => break Some(e),
        }
    };
    let ended = ended.ok_or("the copy read as complete")?;
    assert!(ended.to_string().contains("ended early"), "{ended}");
    Ok(())
}

#[test]
fn an_entry_the_import_would_refuse_fails_the_plan() -> TestResult {
    let dir = tree()?;
    let file = dir.path().join("master.key");
    // Many attributes whose pax records pass the reader's 1 MiB bound.
    let value = vec![b'v'; 60_000];
    for i in 0..20 {
        if rustix::fs::lsetxattr(
            &file,
            format!("user.big{i}").as_str(),
            &value[..],
            rustix::fs::XattrFlags::empty(),
        )
        .is_err()
        {
            eprintln!("skipped: this filesystem keeps no large user attributes");
            return Ok(());
        }
    }
    let err = plan(dir.path()).err().ok_or("expected an error")?;
    assert!(
        err.to_string().contains("more than an export carries"),
        "{err}"
    );
    Ok(())
}

// Passes of a move (SPEC-465).

use std::collections::HashMap;

use crate::instance::Pass;

/// The old host's receipts, in memory: what was sent waits until the new host acknowledges it.
#[derive(Default)]
struct Book {
    held: HashMap<Digest, Digest>,
    sent: Vec<(Digest, Option<Digest>)>,
    kept: Vec<(Digest, Digest)>,
    sealed: u32,
}

impl Ledger for Book {
    fn held(&self, path: &Digest, state: &Digest) -> bool {
        self.held.get(path) == Some(state)
    }

    fn sending(&mut self, path: Digest, _: [u8; 8]) -> io::Result<()> {
        self.sent.push((path, None));
        Ok(())
    }

    fn earned(&mut self, state: Digest) -> io::Result<()> {
        if let Some(last) = self.sent.last_mut() {
            last.1 = Some(state);
        }
        Ok(())
    }

    fn kept(&mut self, path: Digest, state: Digest) -> io::Result<()> {
        self.kept.push((path, state));
        Ok(())
    }

    fn seal(&mut self) -> io::Result<()> {
        self.sealed += 1;
        Ok(())
    }
}

impl Book {
    /// The new host acknowledged the stream: its receipts count.
    fn acknowledge(&mut self) {
        for (path, state) in self.sent.drain(..) {
            match state {
                Some(state) => self.held.insert(path, state),
                None => self.held.remove(&path),
            };
        }
        self.kept.clear();
    }
}

fn pass(live: bool) -> Manifest {
    Manifest {
        pass: Some(Pass {
            stage: "s1".into(),
            nonce: "n".into(),
            live,
            ..Pass::default()
        }),
        ..manifest()
    }
}

/// Long after every change: each stable entry earns its receipt.
fn later() -> (i64, i64) {
    (i64::MAX / 2, 0)
}

/// What a stream holds: its manifest, the paths of its entries with their content, the kept
/// list.
struct Stream {
    manifest: Manifest,
    entries: Vec<(String, Kind, Vec<u8>)>,
    kept: Vec<Digest>,
}

impl Stream {
    fn paths(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.0.as_str()).collect()
    }

    fn content(&self, path: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|e| e.0 == path)
            .map(|e| e.2.as_slice())
    }
}

fn read(bytes: &[u8]) -> Result<Stream, Box<dyn std::error::Error>> {
    use std::io::Read as _;
    let mut r = Reader::new(bytes);
    let Item::Entry(first) = r.next_item()? else {
        return Err("no manifest".into());
    };
    assert_eq!(first.path, MANIFEST);
    let mut json = Vec::new();
    r.data_reader().read_to_end(&mut json)?;
    let mut stream = Stream {
        manifest: serde_json::from_slice(&json)?,
        entries: Vec::new(),
        kept: Vec::new(),
    };
    while let Item::Entry(e) = r.next_item()? {
        let mut data = Vec::new();
        r.data_reader().read_to_end(&mut data)?;
        if e.path == KEPT {
            stream.kept = data.as_chunks::<16>().0.to_vec();
        } else {
            stream
                .entries
                .push((String::from_utf8(e.path)?, e.kind, data));
        }
    }
    Ok(stream)
}

fn live(
    root: &Path,
    book: &mut Book,
    began: (i64, i64),
) -> Result<(Stream, Sent), Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    let sent = send_live(root, &pass(true), &mut out, book, began, &mut |_| {})?;
    assert_eq!(sent.bytes, out.len() as u64);
    Ok((read(&out)?, sent))
}

/// A warm pass acknowledged whole.
fn warm(root: &Path, book: &mut Book) -> Result<Stream, Box<dyn std::error::Error>> {
    let (stream, _) = live(root, book, later())?;
    book.acknowledge();
    Ok(stream)
}

fn exact(root: &Path, book: &mut Book) -> Result<Stream, Box<dyn std::error::Error>> {
    let plan = plan_pass(root, Some(&*book), &mut |_| {})?;
    let mut out = Vec::new();
    let total = send_pass(
        root,
        &plan,
        &pass(false),
        &mut out,
        Some(book),
        &mut |_| {},
        &mut || Ok(()),
    )?;
    assert_eq!((total, out.len() as u64), (plan.total, plan.total));
    let stream = read(&out)?;
    assert_eq!(stream.manifest.total_bytes, plan.total);
    assert_eq!(stream.manifest.entries, stream.entries.len() as u64);
    Ok(stream)
}

#[test]
fn a_first_warm_pass_sends_everything_and_the_next_only_what_changed() -> TestResult {
    let dir = tree()?;
    let root = dir.path();
    let mut book = Book::default();
    let (first, sent) = live(root, &mut book, later())?;
    assert!(first.manifest.pass.as_ref().is_some_and(|p| p.live));
    assert_eq!(first.paths().len(), 8, "{:?}", first.paths());
    assert_eq!(
        (sent.walked, sent.sent, sent.vanished, sent.torn),
        (8, 8, 0, 0)
    );
    // The records were made durable before the checksum went out.
    assert_eq!(book.sealed, 1);
    assert!(!first.paths().iter().any(|p| p.starts_with("data/system")));
    // Sent is not held: without the new host's word everything is sent again.
    let (again, _) = live(root, &mut Book::default(), later())?;
    assert_eq!(again.paths().len(), 8);
    book.acknowledge();

    let unchanged = warm(root, &mut book)?;
    assert!(unchanged.paths().is_empty(), "{:?}", unchanged.paths());

    fs::write(root.join("master.key"), b"K")?;
    fs::write(root.join("sandboxes/sb/home/new"), b"new")?;
    let second = warm(root, &mut book)?;
    // The file that changed, the new one, and the directory that got it.
    assert_eq!(
        second.paths(),
        [
            "data/master.key",
            "data/sandboxes/sb/home",
            "data/sandboxes/sb/home/new"
        ]
    );
    assert_eq!(second.content("data/master.key"), Some(&b"K"[..]));
    assert!(warm(root, &mut book)?.paths().is_empty());
    Ok(())
}

#[test]
fn the_same_size_and_time_with_other_content_is_sent_again() -> TestResult {
    let dir = tree()?;
    let root = dir.path();
    let key = root.join("master.key");
    let mut book = Book::default();
    warm(root, &mut book)?;
    let before = fs::metadata(&key)?;
    // Rewritten in place with its time set back: size and time say nothing changed.
    fs::write(&key, b"Z")?;
    fs::File::options()
        .write(true)
        .open(&key)?
        .set_modified(before.modified()?)?;
    let after = fs::metadata(&key)?;
    assert_eq!((after.len(), after.mtime()), (before.len(), before.mtime()));
    let second = warm(root, &mut book)?;
    assert_eq!(second.paths(), ["data/master.key"]);
    assert_eq!(second.content("data/master.key"), Some(&b"Z"[..]));

    // Its time alone set back: an attribute of the entry, sent again.
    fs::File::options()
        .write(true)
        .open(&key)?
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000))?;
    assert_eq!(warm(root, &mut book)?.paths(), ["data/master.key"]);
    Ok(())
}

#[test]
fn an_entry_changed_too_close_to_its_pass_earns_no_receipt() -> TestResult {
    let dir = tree()?;
    let root = dir.path();
    let mut book = Book::default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let began = (i64::try_from(now.as_secs())?, i64::from(now.subsec_nanos()));
    // The tree was written this very second: a rewrite within the clock's tick would not show.
    live(root, &mut book, began)?;
    book.acknowledge();
    assert!(book.held.is_empty());
    assert_eq!(live(root, &mut book, began)?.0.paths().len(), 8);
    // Two seconds later they are old enough.
    live(root, &mut book, (began.0 + RACY_SECS + 1, began.1))?;
    book.acknowledge();
    assert_eq!(book.held.len(), 8);
    Ok(())
}

#[test]
fn a_path_whose_directory_was_swapped_away_and_back_is_sent_again() -> TestResult {
    let dir = tree()?;
    let root = dir.path();
    let home = root.join("sandboxes/sb/home");
    fs::write(home.join("f"), b"old!")?;
    let mut book = Book::default();
    warm(root, &mut book)?;

    // Another directory takes the path, with another file of the same size.
    fs::rename(&home, root.join("sandboxes/sb/home.away"))?;
    fs::create_dir(&home)?;
    fs::write(home.join("f"), b"new!")?;
    let between = warm(root, &mut book)?;
    assert_eq!(
        between.content("data/sandboxes/sb/home/f"),
        Some(&b"new!"[..])
    );

    // The first directory comes back: its file was never touched, and the new host holds the
    // other one at that path.
    fs::remove_dir_all(&home)?;
    fs::rename(root.join("sandboxes/sb/home.away"), &home)?;
    let back = warm(root, &mut book)?;
    assert_eq!(back.content("data/sandboxes/sb/home/f"), Some(&b"old!"[..]));
    Ok(())
}

#[test]
fn a_file_that_shrinks_while_it_is_read_is_padded_and_sent_again() -> TestResult {
    let dir = tree()?;
    let root = dir.path().to_path_buf();
    let db = root.join("yulius.db");
    fs::write(&db, vec![7u8; 600_000])?;
    let mut book = Book::default();
    let mut out = Vec::new();
    let mut cut = false;
    let sent = send_live(
        &root,
        &pass(true),
        &mut out,
        &mut book,
        later(),
        // After the first block of the one file that has more than one: it loses its end.
        &mut |written| {
            if !cut && written >= 262_144 {
                cut = fs::File::options()
                    .write(true)
                    .open(&db)
                    .and_then(|f| f.set_len(300_000))
                    .is_ok();
            }
        },
    )?;
    assert_eq!(sent.torn, 1);
    let stream = read(&out)?;
    let data = stream.content("data/yulius.db").ok_or("not sent")?;
    assert_eq!(data.len(), 600_000);
    assert!(data[..262_144].iter().all(|&b| b == 7) && data[300_000..].iter().all(|&b| b == 0));
    book.acknowledge();
    // No receipt: the next pass sends the file as it is now.
    let second = warm(&root, &mut book)?;
    assert_eq!(second.paths(), ["data/yulius.db"]);
    assert_eq!(
        second.content("data/yulius.db").map(<[u8]>::len),
        Some(300_000)
    );
    Ok(())
}

#[test]
fn a_path_that_vanishes_during_a_warm_pass_does_not_fail_it() -> TestResult {
    let dir = tree()?;
    let root = dir.path().to_path_buf();
    fs::write(root.join("sandboxes/sb/home/gone"), b"x")?;
    let mut book = Book::default();
    let mut out = Vec::new();
    let mut files = 0;
    let sent = send_live(
        &root,
        &pass(true),
        &mut out,
        &mut book,
        later(),
        // While the second file is read (`.claude/s.jsonl`), two files the walk has listed
        // and not looked at yet go away: one beside it, one at the top.
        &mut |_| {
            files += 1;
            if files == 2 {
                let _ = fs::remove_file(root.join("sandboxes/sb/home/gone"));
                let _ = fs::remove_file(root.join("yulius.db"));
            }
        },
    )?;
    let stream = read(&out)?;
    assert_eq!(sent.vanished, 2, "{:?}", stream.paths());
    assert!(
        !stream
            .paths()
            .iter()
            .any(|p| p.ends_with("gone") || p.ends_with("yulius.db"))
    );
    assert!(
        stream
            .paths()
            .contains(&"data/sandboxes/sb/home/.claude/s.jsonl")
    );

    // An exact pass does not tolerate it.
    fs::write(root.join("yulius.db"), b"db")?;
    let plan = plan_pass(&root, Some(&book), &mut |_| {})?;
    fs::remove_file(root.join("yulius.db"))?;
    let err = send_pass(
        &root,
        &plan,
        &pass(false),
        Vec::new(),
        Some(&mut book),
        &mut |_| {},
        &mut || Ok(()),
    );
    assert!(err.is_err());
    Ok(())
}

#[test]
fn what_took_a_file_s_place_is_never_followed_or_waited_on() -> TestResult {
    // Between the walk's look at a file and its open, the path may have become something
    // else; the open is the guard.
    let dir = tempfile::tempdir()?;
    fs::write(dir.path().join("secret"), b"not the instance's")?;
    std::os::unix::fs::symlink(dir.path().join("secret"), dir.path().join("link"))?;
    let followed = open(&dir.path().join("link"));
    assert!(followed.as_ref().is_err_and(walk::gone), "{followed:?}");
    // A FIFO nobody writes to: not waited on, and not a file.
    nix::unistd::mkfifo(&dir.path().join("fifo"), nix::sys::stat::Mode::S_IRWXU)?;
    let fifo = open(&dir.path().join("fifo"));
    assert!(fifo.as_ref().is_err_and(walk::gone), "{fifo:?}");
    assert!(open(&dir.path().join("missing")).is_err_and(|e| walk::gone(&e)));
    let file = open(&dir.path().join("secret"))?;
    assert!(file.metadata()?.is_file());
    // A file is opened as any program opens it: an open that does not wait would fail on a
    // file somebody holds a lease on, instead of waiting for the lease to be given up.
    #[cfg(target_os = "linux")]
    assert!(!rustix::fs::fcntl_getfl(&file)?.contains(rustix::fs::OFlags::NONBLOCK));
    let mut text = String::new();
    io::Read::read_to_string(&mut &file, &mut text)?;
    assert_eq!(text, "not the instance's");
    // A socket cannot be opened: gone as well.
    let _socket = std::os::unix::net::UnixListener::bind(dir.path().join("sock"))?;
    assert!(open(&dir.path().join("sock")).is_err_and(|e| walk::gone(&e)));
    // Anything else names the path it failed at and stays what it was.
    let denied = at(
        b"data/sandboxes/sb/f",
        io::Error::from(io::ErrorKind::PermissionDenied),
    );
    assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        denied.to_string().starts_with("data/sandboxes/sb/f: "),
        "{denied}"
    );
    Ok(())
}

#[test]
fn the_links_of_a_file_travel_together() -> TestResult {
    let dir = tree()?;
    let root = dir.path();
    let home = root.join("sandboxes/sb/home");
    fs::write(home.join("a"), b"one")?;
    fs::hard_link(home.join("a"), home.join("b"))?;
    let mut book = Book::default();
    let first = warm(root, &mut book)?;
    let kind = |s: &Stream, path: &str| s.entries.iter().find(|e| e.0 == path).map(|e| e.1.clone());
    assert_eq!(
        kind(&first, "data/sandboxes/sb/home/a"),
        Some(Kind::File { size: 3 })
    );
    assert_eq!(
        kind(&first, "data/sandboxes/sb/home/b"),
        Some(Kind::HardLink {
            target: b"data/sandboxes/sb/home/a".to_vec()
        })
    );
    assert!(warm(root, &mut book)?.paths().is_empty());

    // Written through one name: both are the same inode, both are sent.
    fs::write(home.join("b"), b"two")?;
    let second = warm(root, &mut book)?;
    assert_eq!(
        second.paths(),
        ["data/sandboxes/sb/home/a", "data/sandboxes/sb/home/b"]
    );
    assert_eq!(
        second.content("data/sandboxes/sb/home/a"),
        Some(&b"two"[..])
    );

    // The first name's receipt alone is gone (its stream was cut before the link): the file
    // is sent again, which makes a new file on the new host, so the link follows it.
    book.held.remove(&path_digest(b"data/sandboxes/sb/home/a"));
    let third = warm(root, &mut book)?;
    assert_eq!(
        third.paths(),
        ["data/sandboxes/sb/home/a", "data/sandboxes/sb/home/b"]
    );
    let frozen = exact(root, &mut book)?;
    assert!(frozen.paths().is_empty());
    book.acknowledge();
    book.held.remove(&path_digest(b"data/sandboxes/sb/home/a"));
    let frozen = exact(root, &mut book)?;
    assert_eq!(
        frozen.paths(),
        ["data/sandboxes/sb/home/a", "data/sandboxes/sb/home/b"]
    );

    // The link broken: two files now.
    book.acknowledge();
    warm(root, &mut book)?;
    fs::remove_file(home.join("b"))?;
    fs::write(home.join("b"), b"own")?;
    let fourth = warm(root, &mut book)?;
    assert_eq!(
        kind(&fourth, "data/sandboxes/sb/home/a"),
        Some(Kind::File { size: 3 })
    );
    assert_eq!(
        kind(&fourth, "data/sandboxes/sb/home/b"),
        Some(Kind::File { size: 3 })
    );
    Ok(())
}

#[test]
fn an_exact_pass_sends_what_is_not_held_and_names_every_path() -> TestResult {
    let dir = tree()?;
    let root = dir.path();
    let mut book = Book::default();
    warm(root, &mut book)?;
    fs::write(root.join("master.key"), b"K2")?;
    fs::remove_file(root.join("sandboxes/sb/home/.claude/s.jsonl"))?;

    let mut seen = 0;
    let plan = plan_pass(root, Some(&book), &mut |_| seen += 1)?;
    // Everything is observed (the inventory), three entries are sent: the file, and the two
    // directories whose time moved.
    assert_eq!((seen, plan.kept, plan.entries), (7, 7, 2));
    let frozen = exact(root, &mut book)?;
    assert_eq!(
        frozen.paths(),
        ["data/master.key", "data/sandboxes/sb/home/.claude"]
    );
    assert_eq!(
        frozen.manifest.pass.as_ref().map(|p| (p.live, p.kept)),
        Some((false, 7))
    );
    let mut kept = frozen.kept.clone();
    kept.sort_unstable();
    let mut expected: Vec<Digest> = [
        "data",
        "data/master.key",
        "data/sandboxes",
        "data/sandboxes/sb",
        "data/sandboxes/sb/home",
        "data/sandboxes/sb/home/.claude",
        "data/yulius.db",
    ]
    .iter()
    .map(|p| path_digest(p.as_bytes()))
    .collect();
    expected.sort_unstable();
    assert_eq!(kept, expected);
    // What it left alone is recorded with its state; what it sent earns nothing while frozen.
    assert_eq!(book.kept.len(), 5);
    assert!(book.sent.iter().all(|(_, state)| state.is_none()));
    assert_eq!(book.sent.len(), 2);
    Ok(())
}

#[test]
fn a_whole_copy_carries_no_pass_and_no_kept_list() -> TestResult {
    let dir = tree()?;
    let plan = plan(dir.path())?;
    assert_eq!(plan.kept, 0);
    let mut out = Vec::new();
    send(
        dir.path(),
        &plan,
        &manifest(),
        &mut out,
        &mut |_| {},
        &mut || Ok(()),
    )?;
    let stream = read(&out)?;
    assert!(stream.manifest.pass.is_none() && stream.kept.is_empty());
    assert_eq!(stream.entries.len() as u64, plan.entries);
    // Its manifest does not name the field at all: an older build reads it as before.
    let json = String::from_utf8(manifest().to_bytes()?)?;
    assert!(!json.contains("pass"), "{json}");
    Ok(())
}

#[test]
fn syncing_the_data_directory_s_filesystem_works_on_a_directory() -> TestResult {
    let dir = tree()?;
    sync(dir.path())?;
    assert!(sync(&dir.path().join("missing")).is_err());
    Ok(())
}

/// A walk over a running instance beside a process that swaps a directory for a link (Linux:
/// the walk holds its directories open there).
#[cfg(target_os = "linux")]
mod swapped {
    use super::*;

    /// A ledger that changes the tree when the pass asks about one path: what a process of a
    /// sandbox may do between two looks of a walk over a running instance.
    struct Meddler<'a> {
        book: Book,
        at: Digest,
        act: std::cell::RefCell<Option<Box<dyn FnOnce() + 'a>>>,
    }

    impl<'a> Meddler<'a> {
        fn at(path: &str, act: impl FnOnce() + 'a) -> Self {
            Self {
                book: Book::default(),
                at: path_digest(path.as_bytes()),
                act: std::cell::RefCell::new(Some(Box::new(act))),
            }
        }

        fn acted(&self) -> bool {
            self.act.borrow().is_none()
        }
    }

    impl Ledger for Meddler<'_> {
        fn held(&self, path: &Digest, state: &Digest) -> bool {
            if *path == self.at
                && let Some(act) = self.act.borrow_mut().take()
            {
                act();
            }
            self.book.held(path, state)
        }

        fn sending(&mut self, path: Digest, check: [u8; 8]) -> io::Result<()> {
            self.book.sending(path, check)
        }

        fn earned(&mut self, state: Digest) -> io::Result<()> {
            self.book.earned(state)
        }

        fn kept(&mut self, path: Digest, state: Digest) -> io::Result<()> {
            self.book.kept(path, state)
        }

        fn seal(&mut self) -> io::Result<()> {
            self.book.seal()
        }
    }

    /// A sandbox's directory `sandboxes/sb/dir` with the files `a` and `b`, and, outside the data
    /// directory, a directory that is not the instance's: the files `a`, `b` and `secret` and the
    /// directory `deep` with one more.
    fn beside_a_secret() -> Result<(tempfile::TempDir, PathBuf, PathBuf), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let (root, outside) = (dir.path().join("data"), dir.path().join("outside"));
        fs::create_dir_all(root.join("sandboxes/sb/dir"))?;
        fs::write(root.join("sandboxes/sb/dir/a"), b"inside a")?;
        fs::write(root.join("sandboxes/sb/dir/b"), b"inside b")?;
        fs::create_dir_all(outside.join("deep"))?;
        for name in ["a", "b", "secret", "deep/more"] {
            fs::write(outside.join(name), b"SECRET of the host")?;
        }
        Ok((dir, root, outside))
    }

    /// The sandbox's directory goes aside and a link to `outside` takes its name.
    fn swap_for_a_link(root: &Path, outside: &Path) {
        let dir = root.join("sandboxes/sb/dir");
        let _ = fs::rename(&dir, root.join("sandboxes/sb/dir.aside"));
        let _ = std::os::unix::fs::symlink(outside, &dir);
    }

    /// Whether [`swap_for_a_link`] happened.
    fn swapped(root: &Path) -> bool {
        fs::symlink_metadata(root.join("sandboxes/sb/dir")).is_ok_and(|meta| meta.is_symlink())
            && root.join("sandboxes/sb/dir.aside/a").is_file()
    }

    fn nothing_of_the_host(stream: &Stream) {
        for (path, _, content) in &stream.entries {
            assert!(
                !content.windows(6).any(|w| w == b"SECRET"),
                "{path} carries a file from outside the data directory"
            );
        }
        assert!(
            !stream
                .paths()
                .iter()
                .any(|p| p.ends_with("secret") || p.contains("deep")),
            "{:?}",
            stream.paths()
        );
    }

    #[test]
    fn a_directory_that_became_a_link_during_a_warm_pass_is_not_entered() -> TestResult {
        // After the walk looked at the directory and before it lists it.
        let (_dir, root, outside) = beside_a_secret()?;
        let mut meddler = Meddler::at("data/sandboxes/sb/dir", || swap_for_a_link(&root, &outside));
        let mut out = Vec::new();
        let sent = send_live(
            &root,
            &pass(true),
            &mut out,
            &mut meddler,
            later(),
            &mut |_| {},
        )?;
        assert!(meddler.acted() && swapped(&root));
        let stream = read(&out)?;
        nothing_of_the_host(&stream);
        assert_eq!(sent.vanished, 1, "{:?}", stream.paths());
        assert!(
            !stream
                .paths()
                .iter()
                .any(|p| p.starts_with("data/sandboxes/sb/dir/"))
        );
        Ok(())
    }

    #[test]
    fn a_file_is_read_in_the_directory_the_walk_listed_whatever_its_name_leads_to_now() -> TestResult
    {
        // After the walk looked at the file `a` and before the pass opens it; `b` is looked at
        // only afterwards.
        let (_dir, root, outside) = beside_a_secret()?;
        let mut meddler = Meddler::at("data/sandboxes/sb/dir/a", || {
            swap_for_a_link(&root, &outside);
        });
        let mut out = Vec::new();
        send_live(
            &root,
            &pass(true),
            &mut out,
            &mut meddler,
            later(),
            &mut |_| {},
        )?;
        assert!(meddler.acted() && swapped(&root));
        let stream = read(&out)?;
        nothing_of_the_host(&stream);
        // What the walk listed is what it read: the directory it holds, not the name.
        assert_eq!(
            stream.content("data/sandboxes/sb/dir/a"),
            Some(&b"inside a"[..])
        );
        assert_eq!(
            stream.content("data/sandboxes/sb/dir/b"),
            Some(&b"inside b"[..])
        );
        Ok(())
    }
}

#[test]
fn a_pass_measures_what_it_sent_without_a_receipt() -> TestResult {
    let dir = tree()?;
    // Long after every change: every entry earns its receipt.
    let (_, sent) = live(dir.path(), &mut Book::default(), later())?;
    assert!(sent.sending > Duration::ZERO);
    assert_eq!(sent.unearned, Duration::ZERO);
    // A pass that began before the files last changed: nothing earns one, so the next pass
    // sends all of it again, whether it changes or not.
    let (_, sent) = live(dir.path(), &mut Book::default(), (0, 0))?;
    assert!(sent.unearned > Duration::ZERO);
    assert_eq!(sent.unearned, sent.sending);
    Ok(())
}
