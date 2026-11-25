//! Lossless handling of file names that are not valid UTF-8.
//!
//! Git reports paths as raw bytes. To keep one `String` per path in the model, bytes that are not
//! valid UTF-8 are mapped to private-use characters (`U+F700 + byte`) and mapped back when the path
//! is handed to git again. On Windows names are UTF-16 and git converts them to UTF-8, so this
//! only matters for repositories created on other systems.

use std::ffi::OsString;

const BASE: u32 = 0xF700;

/// Decodes raw path bytes; invalid bytes become private-use characters.
pub fn decode_path(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                let bad = e.error_len().unwrap_or(after.len()).max(1).min(after.len());
                for &b in &after[..bad] {
                    out.push(char::from_u32(BASE + u32::from(b)).unwrap_or('\u{FFFD}'));
                }
                rest = &after[bad..];
                if rest.is_empty() {
                    return out;
                }
            }
        }
    }
}

fn is_escaped(c: char) -> bool {
    (BASE..BASE + 256).contains(&(c as u32))
}

/// Whether the path contains bytes that were escaped by [`decode_path`].
pub fn has_escaped_bytes(s: &str) -> bool {
    s.chars().any(is_escaped)
}

/// Turns a path from the model back into what git expects.
#[cfg(unix)]
pub fn encode_path(s: &str) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    let mut bytes = Vec::with_capacity(s.len());
    for c in s.chars() {
        if is_escaped(c) {
            bytes.push((c as u32 - BASE) as u8);
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    OsString::from_vec(bytes)
}

/// Turns a path from the model back into what git expects (Windows paths are Unicode already).
#[cfg(not(unix))]
pub fn encode_path(s: &str) -> OsString {
    OsString::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_utf8_is_untouched() {
        assert_eq!(decode_path("dir/café 🦀.txt".as_bytes()), "dir/café 🦀.txt");
        assert!(!has_escaped_bytes("dir/café.txt"));
    }

    #[test]
    fn invalid_bytes_are_escaped_and_marked() {
        let s = decode_path(b"bad-\xff-name\xfe");
        assert!(has_escaped_bytes(&s));
        assert!(s.starts_with("bad-"));
        assert_eq!(s.chars().filter(|c| is_escaped(*c)).count(), 2);
    }

    #[test]
    fn truncated_sequence_at_the_end_is_handled() {
        // 0xE2 0x82 is the start of a three byte sequence.
        let s = decode_path(b"x\xe2\x82");
        assert!(s.starts_with('x'));
        assert_eq!(s.chars().count(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn roundtrip_restores_the_original_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let original: &[u8] = b"a/\xff\xfe/b\xc3\xa9.txt";
        let model = decode_path(original);
        assert_eq!(encode_path(&model).as_bytes(), original);
    }

    #[cfg(not(unix))]
    #[test]
    fn plain_paths_roundtrip() {
        let model = decode_path("a/é.txt".as_bytes());
        assert_eq!(encode_path(&model), OsString::from("a/é.txt"));
    }
}
