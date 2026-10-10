//! The encodings an agent-secret value is recognised in when it is redacted: hexadecimal,
//! base64 and base64url (at any alignment in a longer encoded text), URL (percent) encoding and
//! JSON string escaping (VLT-007).

/// Shortest needle: shorter encoded fragments would match ordinary text.
pub const MIN_NEEDLE_LEN: usize = 8;

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64 of `bytes` (`url`: the URL-safe alphabet; `pad`: `=` padding).
#[must_use]
pub fn base64(bytes: &[u8], url: bool, pad: bool) -> String {
    let alphabet = if url { URL } else { STD };
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let symbols = chunk.len() + 1;
        for i in 0..4 {
            if i < symbols {
                out.push(char::from(alphabet[((n >> (18 - 6 * i)) & 63) as usize]));
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

/// Lowercase or uppercase hexadecimal of `bytes`.
#[must_use]
pub fn hex(bytes: &[u8], upper: bool) -> String {
    let digits: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(digits[usize::from(b >> 4)]));
        out.push(char::from(digits[usize::from(b & 15)]));
    }
    out
}

/// Percent-encoding of everything but RFC 3986 unreserved characters (uppercase hex).
#[must_use]
pub fn percent(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push_str(&hex(&[b], true));
        }
    }
    out
}

/// The body of a JSON string holding `text` (`slash`: also `/` as `\/`; `ascii`: non-ASCII as
/// `\uXXXX`).
#[must_use]
pub fn json_escape(text: &str, slash: bool, ascii: bool) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' if slash => out.push_str("\\/"),
            c if c.is_control() || (ascii && !c.is_ascii()) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Every form of `value` that redaction looks for, each at least [`MIN_NEEDLE_LEN`] bytes and
/// without duplicates: the value; its hexadecimal (both cases); its base64 and base64url
/// inside longer encoded text at each of the three byte alignments (only the symbols the value
/// alone decides, so a match does not depend on neighbouring bytes); its percent-encoding; and
/// its JSON-escaped forms.
#[must_use]
pub fn needles(value: &str) -> Vec<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut add = |needle: Vec<u8>| {
        if needle.len() >= MIN_NEEDLE_LEN && !out.contains(&needle) {
            out.push(needle);
        }
    };
    add(bytes.to_vec());
    add(hex(bytes, false).into_bytes());
    add(hex(bytes, true).into_bytes());
    // The value starting at offset ≡ k (mod 3) in an encoded stream: skip (3 - k) % 3 bytes to
    // reach a group boundary, then encode whole groups only.
    for skip in 0..3 {
        if bytes.len() <= skip {
            continue;
        }
        let aligned = &bytes[skip..];
        let whole = &aligned[..aligned.len() - aligned.len() % 3];
        if whole.is_empty() {
            continue;
        }
        add(base64(whole, false, false).into_bytes());
        add(base64(whole, true, false).into_bytes());
    }
    add(percent(bytes).into_bytes());
    add(json_escape(value, false, false).into_bytes());
    add(json_escape(value, true, false).into_bytes());
    add(json_escape(value, true, true).into_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, padded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes(), false, true), padded);
            assert_eq!(base64(input.as_bytes(), false, false), padded.trim_end_matches('='));
        }
        assert_eq!(base64(&[0xfb, 0xff], false, true), "+/8=");
        assert_eq!(base64(&[0xfb, 0xff], true, true), "-_8=");
    }

    #[test]
    fn hex_percent_and_json() {
        assert_eq!(hex(b"\x01\xab", false), "01ab");
        assert_eq!(hex(b"\x01\xab", true), "01AB");
        assert_eq!(percent(b"a b/c~"), "a%20b%2Fc~");
        assert_eq!(json_escape("a\"b\\c/d", false, false), "a\\\"b\\\\c/d");
        assert_eq!(json_escape("a/ż", true, true), "a\\/\\u017c");
    }

    /// Whatever bytes surround the value, its base64 at every offset contains one needle.
    #[test]
    fn base64_needles_cover_every_alignment() {
        let value = "sk_live_51HxQ9b2eZvKYlo2C";
        let set = needles(value);
        for prefix in ["", "x", "xy", "xyz", "{\"k\":\""] {
            for suffix in ["", "\"}", "q"] {
                let text = format!("{prefix}{value}{suffix}");
                for url in [false, true] {
                    let encoded = base64(text.as_bytes(), url, true);
                    assert!(
                        set.iter().any(|n| encoded.as_bytes().windows(n.len()).any(|w| w == n.as_slice())),
                        "{prefix:?}+value+{suffix:?} url={url}: {encoded}"
                    );
                }
                let hexed = hex(text.as_bytes(), false);
                assert!(set.iter().any(|n| hexed.as_bytes().windows(n.len()).any(|w| w == n.as_slice())));
            }
        }
    }

    #[test]
    fn needles_are_long_enough_and_distinct() {
        let set = needles("abc/def\"gh");
        assert!(set.iter().all(|n| n.len() >= MIN_NEEDLE_LEN));
        let mut sorted = set.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), set.len());
        assert!(set.contains(&b"abc\\/def\\\"gh".to_vec()));
        assert!(set.contains(&b"abc%2Fdef%22gh".to_vec()));
    }
}
