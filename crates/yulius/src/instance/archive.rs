//! The export's archive format (SPEC-329 EXP-004): a POSIX pax tar whose length is known before
//! the first byte. Each entry is a ustar header, preceded by a pax header only when a field does
//! not fit ustar (a long path or link, a large id or size, a negative time, extended
//! attributes), then its data padded to 512 bytes. The archive ends with a SHA-256 of every byte
//! before the trailer's header, as the entry [`TRAILER`], and two zero blocks.
//!
//! [`Writer`] streams it and [`Reader`] reads it back; both are ours, so the import checks the
//! trailer over exactly the bytes it read.

use std::io::{self, Read, Write};

use sha2::{Digest as _, Sha256};

/// tar's block.
pub const BLOCK: u64 = 512;
/// The trailer entry's name.
pub const TRAILER: &[u8] = b"yulius-export.sha256";
/// The trailer's content: 64 hex digits and a newline.
const TRAILER_LEN: u64 = 65;
/// ustar's largest size and time (11 octal digits).
const MAX_OCTAL_11: u64 = 0o77_777_777_777;
/// ustar's largest id (7 octal digits).
const MAX_OCTAL_7: u64 = 0o7_777_777;
/// A pax header's own name.
const PAX_NAME: &[u8] = b"././@PaxHeader";
/// The largest pax header a reader accepts (extended attributes included).
pub const MAX_PAX: u64 = 1 << 20;

/// What an entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A directory.
    Dir,
    /// A regular file of `size` bytes.
    File {
        /// Its length.
        size: u64,
    },
    /// A symbolic link to `target` (kept as it is, never followed).
    Symlink {
        /// The link's text.
        target: Vec<u8>,
    },
    /// A hard link to the earlier entry `target` (an archive path).
    HardLink {
        /// The first path of the same file.
        target: Vec<u8>,
    },
    /// A character device 0:0, an overlay's whiteout.
    Whiteout,
}

/// One archive entry's metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The archive path (`data/…`), without a trailing slash.
    pub path: Vec<u8>,
    /// What it is.
    pub kind: Kind,
    /// Permission bits with setuid, setgid and sticky (`0o7777`).
    pub mode: u32,
    /// Numeric owner.
    pub uid: u32,
    /// Numeric group.
    pub gid: u32,
    /// Modification time in seconds since the epoch.
    pub mtime: i64,
    /// Extended attributes, names with their namespace (`user.x`).
    pub xattrs: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Entry {
    /// The data bytes that follow its headers.
    #[must_use]
    pub fn size(&self) -> u64 {
        match self.kind {
            Kind::File { size } => size,
            _ => 0,
        }
    }

    /// Its whole length in the archive: headers and padded data.
    #[must_use]
    pub fn archived_len(&self) -> u64 {
        headers(self).len() as u64 + padded(self.size())
    }
}

/// `n` rounded up to whole blocks.
#[must_use]
pub fn padded(n: u64) -> u64 {
    n.div_ceil(BLOCK) * BLOCK
}

/// The length of a regular file entry of `size` bytes at a short path with no extensions (the
/// manifest).
#[must_use]
pub fn plain_file_len(size: u64) -> u64 {
    BLOCK + padded(size)
}

/// What the archive adds after the last entry: the trailer and the end blocks.
#[must_use]
pub fn ending_len() -> u64 {
    plain_file_len(TRAILER_LEN) + 2 * BLOCK
}

/// One pax record, `"<len> <key>=<value>\n"`, its length counting its own digits.
fn pax_record(out: &mut Vec<u8>, key: &[u8], value: &[u8]) {
    let base = key.len() + value.len() + 3;
    let mut len = base + 1;
    while base + len.to_string().len() != len {
        len += 1;
    }
    out.extend_from_slice(len.to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(key);
    out.push(b'=');
    out.extend_from_slice(value);
    out.push(b'\n');
}

/// Splits `path` into ustar's prefix (155) and name (100), when it fits.
fn ustar_split(path: &[u8]) -> Option<(&[u8], &[u8])> {
    if path.len() <= 100 {
        return Some((&[], path));
    }
    (1..path.len())
        .rev()
        .find(|&i| path[i] == b'/' && i <= 155 && path.len() - i - 1 <= 100)
        .map(|i| (&path[..i], &path[i + 1..]))
}

/// Writes `value` as `width - 1` octal digits and a NUL.
fn octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let text = format!("{value:0width$o}");
    field[..width].copy_from_slice(&text.as_bytes()[text.len() - width..]);
    field[width] = 0;
}

/// A ustar header block.
fn ustar(name: &[u8], prefix: &[u8], kind: u8, e: &Entry, size: u64, link: &[u8]) -> [u8; 512] {
    let mut h = [0u8; 512];
    let cut = |src: &[u8], n: usize| src[..src.len().min(n)].to_vec();
    h[..name.len().min(100)].copy_from_slice(&cut(name, 100));
    octal(&mut h[100..108], u64::from(e.mode & 0o7777));
    octal(&mut h[108..116], u64::from(e.uid).min(MAX_OCTAL_7));
    octal(&mut h[116..124], u64::from(e.gid).min(MAX_OCTAL_7));
    octal(&mut h[124..136], size.min(MAX_OCTAL_11));
    octal(
        &mut h[136..148],
        u64::try_from(e.mtime).unwrap_or(0).min(MAX_OCTAL_11),
    );
    h[156] = kind;
    h[157..157 + link.len().min(100)].copy_from_slice(&cut(link, 100));
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    octal(&mut h[329..337], 0);
    octal(&mut h[337..345], 0);
    h[345..345 + prefix.len().min(155)].copy_from_slice(&cut(prefix, 155));
    h[148..156].copy_from_slice(b"        ");
    let sum: u64 = h.iter().map(|&b| u64::from(b)).sum();
    octal(&mut h[148..155], sum);
    h[155] = b' ';
    h
}

/// Refuses an entry whose extended header the reader would refuse ([`MAX_PAX`]), so an export
/// fails before its first byte rather than at the import.
///
/// # Errors
/// The entry's extended attributes or names are too large.
pub fn check_entry(e: &Entry) -> io::Result<()> {
    let len = headers(e).len() as u64;
    // A pax header is its own block plus its padded records, before the entry's block.
    if len > 2 * BLOCK + MAX_PAX {
        return Err(io::Error::other(format!(
            "{}: its names and extended attributes take {len} bytes, more than an export carries",
            String::from_utf8_lossy(&e.path)
        )));
    }
    Ok(())
}

/// The header blocks of `e`: a pax header when needed, then its ustar header.
#[must_use]
pub fn headers(e: &Entry) -> Vec<u8> {
    let (kind, link): (u8, &[u8]) = match &e.kind {
        Kind::Dir => (b'5', b""),
        Kind::File { .. } => (b'0', b""),
        Kind::Symlink { target } => (b'2', target),
        Kind::HardLink { target } => (b'1', target),
        Kind::Whiteout => (b'3', b""),
    };
    let size = e.size();
    let mut pax = Vec::new();
    let split = ustar_split(&e.path);
    if split.is_none() || !e.path.is_ascii() {
        pax_record(&mut pax, b"path", &e.path);
    }
    if link.len() > 100 || !link.is_ascii() {
        pax_record(&mut pax, b"linkpath", link);
    }
    if u64::from(e.uid) > MAX_OCTAL_7 {
        pax_record(&mut pax, b"uid", e.uid.to_string().as_bytes());
    }
    if u64::from(e.gid) > MAX_OCTAL_7 {
        pax_record(&mut pax, b"gid", e.gid.to_string().as_bytes());
    }
    if size > MAX_OCTAL_11 {
        pax_record(&mut pax, b"size", size.to_string().as_bytes());
    }
    if e.mtime < 0 || e.mtime.unsigned_abs() > MAX_OCTAL_11 {
        pax_record(&mut pax, b"mtime", e.mtime.to_string().as_bytes());
    }
    for (name, value) in &e.xattrs {
        let mut key = b"SCHILY.xattr.".to_vec();
        key.extend_from_slice(name);
        pax_record(&mut pax, &key, value);
    }
    let mut out = Vec::with_capacity(1024);
    if !pax.is_empty() {
        let len = pax.len() as u64;
        let pax_entry = Entry {
            path: PAX_NAME.to_vec(),
            kind: Kind::File { size: len },
            mode: 0o644,
            uid: 0,
            gid: 0,
            mtime: 0,
            xattrs: Vec::new(),
        };
        out.extend_from_slice(&ustar(PAX_NAME, b"", b'x', &pax_entry, len, b""));
        out.extend_from_slice(&pax);
        out.resize(out.len() + (padded(len) - len) as usize, 0);
    }
    let (prefix, name) = split.unwrap_or((b"", &e.path));
    out.extend_from_slice(&ustar(name, prefix, kind, e, size, link));
    out
}

/// Streams an archive and counts what it wrote.
pub struct Writer<W: Write> {
    out: W,
    hash: Sha256,
    written: u64,
}

impl<W: Write> Writer<W> {
    /// A writer over `out`.
    pub fn new(out: W) -> Self {
        Self {
            out,
            hash: Sha256::new(),
            written: 0,
        }
    }

    /// Bytes written so far.
    #[must_use]
    pub fn written(&self) -> u64 {
        self.written
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.hash.update(bytes);
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Writes `e` with exactly `e.size()` bytes from `data` (a file's content).
    ///
    /// # Errors
    /// A write error, or `data` holding fewer or more bytes than the entry says (the file
    /// changed since it was planned).
    pub fn entry(
        &mut self,
        e: &Entry,
        data: Option<&mut dyn Read>,
        mut tick: impl FnMut(u64),
    ) -> io::Result<()> {
        self.put(&headers(e))?;
        let size = e.size();
        if size == 0 {
            return Ok(());
        }
        let data = data.ok_or_else(|| io::Error::other("a file entry needs its data"))?;
        let mut left = size;
        let mut buf = vec![0u8; 256 * 1024];
        while left > 0 {
            let want = usize::try_from(left.min(buf.len() as u64)).unwrap_or(buf.len());
            let n = data.read(&mut buf[..want])?;
            if n == 0 {
                return Err(changed(&e.path, "shrank"));
            }
            self.put(&buf[..n])?;
            left -= n as u64;
            tick(self.written);
        }
        if data.read(&mut buf[..1])? != 0 {
            return Err(changed(&e.path, "grew"));
        }
        let pad = padded(size) - size;
        self.put(&vec![0; usize::try_from(pad).unwrap_or(0)])
    }

    /// Writes the trailer and the end blocks, then flushes; returns the archive's length.
    ///
    /// # Errors
    /// A write error.
    pub fn finish(mut self) -> io::Result<u64> {
        let digest = hex::encode(self.hash.clone().finalize());
        let content = format!("{digest}\n");
        let trailer = trailer_entry();
        self.put(&headers(&trailer))?;
        self.put(content.as_bytes())?;
        self.put(&vec![
            0;
            usize::try_from(padded(TRAILER_LEN) - TRAILER_LEN)
                .unwrap_or(0)
        ])?;
        self.put(&[0; 1024])?;
        self.out.flush()?;
        Ok(self.written)
    }
}

fn trailer_entry() -> Entry {
    Entry {
        path: TRAILER.to_vec(),
        kind: Kind::File { size: TRAILER_LEN },
        mode: 0o644,
        uid: 0,
        gid: 0,
        mtime: 0,
        xattrs: Vec::new(),
    }
}

fn changed(path: &[u8], how: &str) -> io::Error {
    io::Error::other(format!(
        "{} {how} during the export; nothing should write while the instance is frozen",
        String::from_utf8_lossy(path)
    ))
}

/// A numeric field: octal text, or base-256 when its first byte has the high bit.
fn number(field: &[u8]) -> io::Result<u64> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        let mut value: u64 = u64::from(field[0] & 0x7f);
        for &b in &field[1..] {
            value = value
                .checked_mul(256)
                .and_then(|v| v.checked_add(u64::from(b)))
                .ok_or_else(|| bad("a numeric field overflows"))?;
        }
        return Ok(value);
    }
    let text: Vec<u8> = field
        .iter()
        .copied()
        .take_while(|&b| b != 0)
        .filter(|&b| b != b' ')
        .collect();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(
        std::str::from_utf8(&text).map_err(|_| bad("a bad number"))?,
        8,
    )
    .map_err(|_| bad("a bad number"))
}

fn bad(why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("not a Yulius export: {why}"),
    )
}

fn nul_cut(field: &[u8]) -> &[u8] {
    field
        .iter()
        .position(|&b| b == 0)
        .map_or(field, |i| &field[..i])
}

/// What [`Reader::next_item`] found.
#[derive(Debug)]
pub enum Item {
    /// An entry; a file's data follows and must be read with [`Reader::data`].
    Entry(Entry),
    /// The trailer matched the bytes before it and the archive ended.
    End,
}

/// Reads an archive written by [`Writer`], hashing what it reads.
pub struct Reader<R: Read> {
    input: R,
    hash: Sha256,
    read: u64,
    /// Data bytes of the current file entry not read yet.
    left: u64,
    /// Padding after them.
    pad: u64,
}

impl<R: Read> Reader<R> {
    /// A reader over `input`.
    pub fn new(input: R) -> Self {
        Self {
            input,
            hash: Sha256::new(),
            read: 0,
            left: 0,
            pad: 0,
        }
    }

    /// Bytes read so far.
    #[must_use]
    pub fn read_bytes(&self) -> u64 {
        self.read
    }

    fn take(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.input.read_exact(buf).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the stream ended early (an incomplete copy)",
                )
            } else {
                e
            }
        })?;
        self.hash.update(&*buf);
        self.read += buf.len() as u64;
        Ok(())
    }

    fn skip_rest(&mut self) -> io::Result<()> {
        let mut sink = [0u8; 8192];
        while self.left + self.pad > 0 {
            let n = usize::try_from((self.left + self.pad).min(sink.len() as u64)).unwrap_or(0);
            self.take(&mut sink[..n])?;
            let from_data = (n as u64).min(self.left);
            self.left -= from_data;
            self.pad -= n as u64 - from_data;
        }
        Ok(())
    }

    /// Reads up to `buf.len()` bytes of the current file's data; 0 at its end.
    ///
    /// # Errors
    /// A read error or an early end.
    pub fn data(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let n = usize::try_from(self.left.min(buf.len() as u64)).unwrap_or(buf.len());
        self.take(&mut buf[..n])?;
        self.left -= n as u64;
        Ok(n)
    }

    /// The current file's data as a reader.
    pub fn data_reader(&mut self) -> EntryData<'_, R> {
        EntryData(self)
    }

    /// The next entry; the rest of the previous one is skipped.
    ///
    /// # Errors
    /// A read error, a malformed header, an unknown type, or a trailer that does not match.
    pub fn next_item(&mut self) -> io::Result<Item> {
        self.skip_rest()?;
        let mut pax: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        loop {
            let before = self.hash.clone();
            let mut h = [0u8; 512];
            self.take(&mut h)?;
            if h.iter().all(|&b| b == 0) {
                return Err(bad("it ended without its checksum"));
            }
            let stored = number(&h[148..156])?;
            let mut copy = h;
            copy[148..156].copy_from_slice(b"        ");
            if copy.iter().map(|&b| u64::from(b)).sum::<u64>() != stored {
                return Err(bad("a header's checksum is wrong"));
            }
            let size = number(&h[124..136])?;
            let kind = h[156];
            if kind == b'x' {
                if size > MAX_PAX {
                    return Err(bad("an extended header is too large"));
                }
                let mut records = vec![0u8; usize::try_from(padded(size)).unwrap_or(0)];
                self.take(&mut records)?;
                records.truncate(usize::try_from(size).unwrap_or(0));
                pax = parse_pax(&records)?;
                continue;
            }
            let name = nul_cut(&h[..100]);
            let prefix = nul_cut(&h[345..500]);
            let mut path = if prefix.is_empty() {
                name.to_vec()
            } else {
                [prefix, b"/", name].concat()
            };
            let mut link = nul_cut(&h[157..257]).to_vec();
            let mut e = Entry {
                path: Vec::new(),
                kind: Kind::Dir,
                mode: u32::try_from(number(&h[100..108])? & 0o7777).unwrap_or(0),
                uid: u32::try_from(number(&h[108..116])?).map_err(|_| bad("a large uid"))?,
                gid: u32::try_from(number(&h[116..124])?).map_err(|_| bad("a large gid"))?,
                mtime: i64::try_from(number(&h[136..148])?).unwrap_or(0),
                xattrs: Vec::new(),
            };
            let mut size = size;
            for (key, value) in pax.drain(..) {
                let text = || String::from_utf8_lossy(&value).into_owned();
                match key.as_slice() {
                    b"path" => path = value,
                    b"linkpath" => link = value,
                    b"uid" => e.uid = text().parse().map_err(|_| bad("a bad uid"))?,
                    b"gid" => e.gid = text().parse().map_err(|_| bad("a bad gid"))?,
                    b"size" => size = text().parse().map_err(|_| bad("a bad size"))?,
                    b"mtime" => e.mtime = text().parse().map_err(|_| bad("a bad mtime"))?,
                    k if k.starts_with(b"SCHILY.xattr.") => {
                        e.xattrs.push((k[13..].to_vec(), value));
                    }
                    _ => {}
                }
            }
            e.path = path;
            e.kind = match kind {
                b'5' => Kind::Dir,
                b'0' | 0 => Kind::File { size },
                b'2' => Kind::Symlink { target: link },
                b'1' => Kind::HardLink { target: link },
                b'3' if number(&h[329..337])? == 0 && number(&h[337..345])? == 0 => Kind::Whiteout,
                other => {
                    return Err(bad(&format!("an entry of type {:?}", char::from(other))));
                }
            };
            if e.path == TRAILER {
                return self.check_trailer(&before, size).map(|()| Item::End);
            }
            self.left = e.size();
            self.pad = padded(self.left) - self.left;
            return Ok(Item::Entry(e));
        }
    }

    fn check_trailer(&mut self, before: &Sha256, size: u64) -> io::Result<()> {
        if size != TRAILER_LEN {
            return Err(bad("its checksum entry is malformed"));
        }
        let expected = hex::encode(before.clone().finalize());
        let mut content = vec![0u8; usize::try_from(padded(TRAILER_LEN)).unwrap_or(0)];
        self.take(&mut content)?;
        if content[..64] != *expected.as_bytes() {
            return Err(bad(
                "its checksum does not match what was read (the copy is damaged)",
            ));
        }
        let mut end = [0u8; 1024];
        self.take(&mut end)?;
        if end.iter().any(|&b| b != 0) {
            return Err(bad("something follows its checksum"));
        }
        Ok(())
    }
}

/// The current entry's data, from [`Reader::data_reader`].
pub struct EntryData<'a, R: Read>(&'a mut Reader<R>);

impl<R: Read> Read for EntryData<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.data(buf)
    }
}

/// Parses pax records.
fn parse_pax(mut records: &[u8]) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut out = Vec::new();
    while !records.is_empty() {
        let space = records
            .iter()
            .position(|&b| b == b' ')
            .ok_or_else(|| bad("a bad extended record"))?;
        let len: usize = std::str::from_utf8(&records[..space])
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&l| l > space + 2 && l <= records.len())
            .ok_or_else(|| bad("a bad extended record length"))?;
        let record = &records[space + 1..len - 1];
        let eq = record
            .iter()
            .position(|&b| b == b'=')
            .ok_or_else(|| bad("a bad extended record"))?;
        out.push((record[..eq].to_vec(), record[eq + 1..].to_vec()));
        records = &records[len..];
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
