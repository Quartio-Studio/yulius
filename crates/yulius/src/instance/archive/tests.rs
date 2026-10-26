use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn entry(path: &[u8], kind: Kind) -> Entry {
    Entry {
        path: path.to_vec(),
        kind,
        mode: 0o755,
        uid: 1000,
        gid: 1000,
        mtime: 1_760_000_000,
        xattrs: Vec::new(),
    }
}

fn content(e: &Entry) -> Vec<u8> {
    (0..e.size()).map(|i| (i % 251) as u8).collect()
}

/// Every shape the export writes: plain and long paths, links, large ids, binary attributes.
fn sample() -> Vec<Entry> {
    let deep = [b"data/".as_slice(), &[b'd'; 120], b"/", &[b'f'; 90]].concat();
    let unsplittable = [b"data/".as_slice(), &[b'x'; 300]].concat();
    let mut big_ids = entry(
        b"data/sysroots/sb/upper/etc/passwd",
        Kind::File { size: 513 },
    );
    big_ids.uid = 3_000_000;
    big_ids.gid = 2_097_152;
    big_ids.mtime = -5;
    big_ids.mode = 0o4755;
    let mut attrs = entry(b"data/sysroots/sb/docker/l", Kind::Dir);
    attrs.xattrs = vec![
        (b"trusted.overlay.opaque".to_vec(), b"y".to_vec()),
        (b"user.bin".to_vec(), vec![0, 1, b'\n', b'=', 255]),
    ];
    vec![
        entry(b"data", Kind::Dir),
        entry(b"data/empty", Kind::File { size: 0 }),
        entry(b"data/one", Kind::File { size: 1 }),
        entry(b"data/block", Kind::File { size: 512 }),
        entry(&deep, Kind::File { size: 700 }),
        entry(&unsplittable, Kind::File { size: 3 }),
        entry("data/zażółć".as_bytes(), Kind::File { size: 4 }),
        entry(
            b"data/link",
            Kind::Symlink {
                target: [b'/'; 150].to_vec(),
            },
        ),
        entry(
            b"data/hard",
            Kind::HardLink {
                target: b"data/one".to_vec(),
            },
        ),
        entry(b"data/wh", Kind::Whiteout),
        big_ids,
        attrs,
    ]
}

fn archive(entries: &[Entry]) -> io::Result<(Vec<u8>, u64)> {
    let mut out = Vec::new();
    let mut w = Writer::new(&mut out);
    for e in entries {
        let data = content(e);
        w.entry(e, Some(&mut data.as_slice()), |_| {})?;
    }
    let total = w.finish()?;
    Ok((out, total))
}

#[test]
fn the_planned_length_is_the_written_length() -> TestResult {
    let entries = sample();
    let planned: u64 = entries.iter().map(Entry::archived_len).sum::<u64>() + ending_len();
    let (bytes, total) = archive(&entries)?;
    assert_eq!(total, planned);
    assert_eq!(bytes.len() as u64, planned);
    assert_eq!(bytes.len() as u64 % BLOCK, 0);
    Ok(())
}

#[test]
fn a_short_plain_file_needs_no_extended_header() -> TestResult {
    let e = entry(b"yulius-export.json", Kind::File { size: 4096 });
    assert_eq!(e.archived_len(), plain_file_len(4096));
    Ok(())
}

#[test]
fn what_is_written_reads_back_the_same() -> TestResult {
    let entries = sample();
    let (bytes, _) = archive(&entries)?;
    let mut r = Reader::new(bytes.as_slice());
    for want in &entries {
        let Item::Entry(got) = r.next_item()? else {
            panic!("ended before {:?}", String::from_utf8_lossy(&want.path));
        };
        assert_eq!(&got, want);
        let mut data = Vec::new();
        let mut buf = [0u8; 100];
        loop {
            let n = r.data(&mut buf)?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }
        assert_eq!(data, content(want));
    }
    assert!(matches!(r.next_item()?, Item::End));
    assert_eq!(r.read_bytes(), bytes.len() as u64);
    Ok(())
}

#[test]
fn unread_data_is_skipped_by_the_next_entry() -> TestResult {
    let entries = sample();
    let (bytes, _) = archive(&entries)?;
    let mut r = Reader::new(bytes.as_slice());
    let mut seen = 0;
    while let Item::Entry(_) = r.next_item()? {
        seen += 1;
    }
    assert_eq!(seen, entries.len());
    Ok(())
}

fn read_all(bytes: &[u8]) -> io::Result<()> {
    let mut r = Reader::new(bytes);
    loop {
        if let Item::End = r.next_item()? {
            return Ok(());
        }
    }
}

#[test]
fn a_changed_byte_fails_the_checksum() -> TestResult {
    let (mut bytes, _) = archive(&sample())?;
    // Inside `data/one`'s data block, which no header checksum covers.
    let at = bytes
        .windows(8)
        .position(|w| w == b"data/one")
        .map(|i| i + 512)
        .ok_or("no entry")?;
    bytes[at] ^= 0xff;
    let err = read_all(&bytes).err().ok_or("expected an error")?;
    assert!(err.to_string().contains("damaged"), "{err}");
    Ok(())
}

#[test]
fn a_cut_stream_is_an_incomplete_copy() -> TestResult {
    let (bytes, _) = archive(&sample())?;
    for cut in [100, 1024, bytes.len() - 1600, bytes.len() - 1] {
        let err = read_all(&bytes[..cut]).err().ok_or("expected an error")?;
        assert!(
            err.to_string().contains("ended early") || err.to_string().contains("damaged"),
            "{cut}: {err}"
        );
    }
    Ok(())
}

#[test]
fn an_archive_without_its_checksum_is_refused() -> TestResult {
    let mut out = Vec::new();
    let mut w = Writer::new(&mut out);
    let e = entry(b"data", Kind::Dir);
    w.entry(&e, None, |_| {})?;
    drop(w);
    out.extend_from_slice(&[0; 1024]);
    let err = read_all(&out).err().ok_or("expected an error")?;
    assert!(err.to_string().contains("without its checksum"), "{err}");
    Ok(())
}

#[test]
fn a_file_that_changed_size_aborts_the_entry() -> TestResult {
    let e = entry(b"data/f", Kind::File { size: 10 });
    let mut w = Writer::new(Vec::new());
    let err = w
        .entry(&e, Some(&mut [1u8; 4].as_slice()), |_| {})
        .err()
        .ok_or("expected an error")?;
    assert!(err.to_string().contains("shrank"), "{err}");
    let mut w = Writer::new(Vec::new());
    let err = w
        .entry(&e, Some(&mut [1u8; 11].as_slice()), |_| {})
        .err()
        .ok_or("expected an error")?;
    assert!(err.to_string().contains("grew"), "{err}");
    Ok(())
}

#[test]
fn the_tar_crate_reads_the_paths_and_kinds() -> TestResult {
    let entries = sample();
    let (bytes, _) = archive(&entries)?;
    let mut ar = tar::Archive::new(bytes.as_slice());
    let got: Vec<Vec<u8>> = ar
        .entries()?
        .map(|e| e.map(|e| e.path_bytes().into_owned()))
        .collect::<io::Result<_>>()?;
    let mut want: Vec<Vec<u8>> = entries.iter().map(|e| e.path.clone()).collect();
    want.push(TRAILER.to_vec());
    assert_eq!(got, want);
    Ok(())
}

#[test]
fn gnu_tar_lists_it() -> TestResult {
    let Ok(probe) = std::process::Command::new("tar").arg("--version").output() else {
        eprintln!("skipped: no tar");
        return Ok(());
    };
    if !String::from_utf8_lossy(&probe.stdout).contains("GNU tar") {
        eprintln!("skipped: not GNU tar");
        return Ok(());
    }
    let (bytes, _) = archive(&sample())?;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("x.tar");
    std::fs::write(&file, &bytes)?;
    let out = std::process::Command::new("tar")
        .arg("-tvf")
        .arg(&file)
        .output()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(listing.contains(&"x".repeat(300)), "{listing}");
    assert!(listing.contains("3000000/2097152"), "{listing}");
    assert!(listing.contains("yulius-export.sha256"), "{listing}");
    Ok(())
}

#[test]
fn an_entry_the_reader_would_refuse_is_refused_when_planned() {
    let mut e = entry(b"data/f", Kind::File { size: 1 });
    assert!(check_entry(&e).is_ok());
    e.xattrs = vec![(b"user.big".to_vec(), vec![b'x'; (MAX_PAX + 1) as usize])];
    assert!(check_entry(&e).is_err());
}
