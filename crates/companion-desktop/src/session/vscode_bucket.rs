//! VS Code 插件会话的历史桶解析：只读地由会话 id 反查窗口文件夹。

use super::url::usable_session_id;
use std::path::{Path, PathBuf};

/// A folder the resolver may name: absolute, and never the filesystem root (a
/// window without a folder is not addressable at the OS level).
#[cfg(any(target_os = "macos", test))]
fn usable_folder(path: &Path) -> bool {
    path.is_absolute() && path != Path::new("/")
}

/// Where the VS Code installs archive the plugin's conversations.
#[cfg(target_os = "macos")]
pub(super) fn vscode_history_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let base = PathBuf::from(home).join("Library/Application Support");
    ["Code", "Code - Insiders"]
        .into_iter()
        .map(|name| {
            base.join(name)
                .join("User/globalStorage/tencent-cloud.coding-copilot/genie-history")
        })
        .collect()
}

/// The bucket whose `conversations/<session>` entry exists, or `None` when the
/// session is not archived under this root. Read-only, and silent on any error.
#[cfg(any(target_os = "macos", test))]
fn bucket_for_session(root: &Path, session: &str) -> Option<String> {
    for entry in std::fs::read_dir(root).ok()? {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        if !entry.path().join("conversations").join(session).is_dir() {
            continue;
        }
        if let Some(bucket) = entry.file_name().to_str() {
            return Some(bucket.to_string());
        }
    }
    None
}

/// What a bucket name resolves to.
#[derive(Debug, PartialEq, Eq)]
#[cfg(any(target_os = "macos", test))]
enum BucketFolder {
    /// A folder that exists on disk.
    Folder(PathBuf),
    /// `/`: the session belongs to a window with no folder.
    RootlessWindow,
    /// Undecodable, truncated past recovery, or naming no existing directory.
    Unknown,
}

/// `sanitizeWorkspaceId(p) = base64(p).replace(/[/+=]/g,'_').substring(0,64)`,
/// mirrored from the plugin — this is how a window folder is spelled as a
/// bucket name, and how a `cwd` can be checked against one.
#[cfg(any(target_os = "macos", test))]
fn sanitize_workspace_id(path: &str) -> String {
    base64_encode(path.as_bytes())
        .chars()
        .map(|byte| {
            if matches!(byte, '/' | '+' | '=') {
                '_'
            } else {
                byte
            }
        })
        .take(64)
        .collect()
}

/// Decodes a bucket name back to its window folder. The name is base64 with
/// `/`, `+` and `=` flattened to `_` and cut at 64 characters, so every variant
/// is tried, and only a decode that re-encodes to the same bucket may win.
#[cfg(any(target_os = "macos", test))]
fn decode_bucket_folder(bucket: &str) -> BucketFolder {
    if !bucket.is_ascii() {
        return BucketFolder::Unknown;
    }
    for candidate in bucket_candidates(bucket) {
        let Ok(text) = std::str::from_utf8(&candidate) else {
            continue;
        };
        // A guess that does not reproduce the bucket would name an unrelated
        // path, so it is never accepted — even when such a path exists.
        if !bucket_matches_folder(bucket, text) {
            continue;
        }
        let path = Path::new(text);
        if path == Path::new("/") {
            return BucketFolder::RootlessWindow;
        }
        if path.is_absolute() && path.is_dir() {
            return BucketFolder::Folder(path.to_path_buf());
        }
    }
    BucketFolder::Unknown
}

/// A bucket is `sanitize_workspace_id(folder)` cut at 64 characters; re-encoding
/// a decoded candidate must therefore reproduce the whole bucket (or its first
/// 64 characters, for a folder long enough to have been cut).
#[cfg(any(target_os = "macos", test))]
fn bucket_matches_folder(bucket: &str, folder: &str) -> bool {
    let encoded = sanitize_workspace_id(folder);
    if bucket.len() == 64 {
        encoded.starts_with(bucket)
    } else {
        encoded == bucket
    }
}

/// The base64 texts a bucket name may stand for: the trailing `_` as padding
/// (1 or 2), then a 64-character name read as a truncated base64, then the same
/// name with one `_` read as `+` — the plugin flattens `/`, `+` and `=` alike,
/// and a `+` appears whenever a `~`, `>` or DEL byte lands on a group boundary.
#[cfg(any(target_os = "macos", test))]
fn bucket_candidates(bucket: &str) -> Vec<Vec<u8>> {
    let mut candidates = Vec::new();
    for padding in 0..=2 {
        if padding > 0 && !bucket.ends_with(&"_".repeat(padding)) {
            break;
        }
        let mut text = desanitize(&bucket[..bucket.len() - padding]);
        text.push_str(&"=".repeat(padding));
        candidates.extend(base64_decode(&text));
    }
    for trimmed in 1..=3 {
        if bucket.len() <= trimmed {
            break;
        }
        candidates.extend(base64_decode(&desanitize(
            &bucket[..bucket.len() - trimmed],
        )));
    }
    for flip in 0..bucket.len() {
        if bucket.as_bytes()[flip] != b'_' {
            continue;
        }
        let text: String = bucket
            .char_indices()
            .map(|(index, byte)| match byte {
                '_' if index == flip => '+',
                '_' => '/',
                other => other,
            })
            .collect();
        candidates.extend(base64_decode(&text));
    }
    candidates
}

/// `_` → `/`: the plugin flattened three base64 characters into one, and away
/// from the padding only `/` can have been there.
#[cfg(any(target_os = "macos", test))]
fn desanitize(text: &str) -> String {
    text.chars()
        .map(|byte| if byte == '_' { '/' } else { byte })
        .collect()
}

/// Standard base64 with optional `=` padding, nothing else. A non-canonical
/// tail is rejected so a wrong interpretation cannot pass as data.
#[cfg(any(target_os = "macos", test))]
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let body = text
        .strip_suffix("==")
        .or_else(|| text.strip_suffix('='))
        .unwrap_or(text);
    if body.contains('=') || body.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    let mut accumulator: u32 = 0;
    let mut bits = 0;
    for byte in body.bytes() {
        let value = base64_value(byte)? as u32;
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    (accumulator == 0).then_some(out)
}

#[cfg(any(target_os = "macos", test))]
fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The plugin's own encoder, mirrored for the same-bucket check below.
#[cfg(any(target_os = "macos", test))]
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(ALPHABET[(group >> 18) as usize & 63] as char);
        out.push(ALPHABET[(group >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(group >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[group as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The window folder for a plugin session, most trustworthy evidence first: the
/// bucket the plugin archived the conversation in, then the hook's `cwd` — but
/// only when the bucket agrees it is the same window, then the bare `cwd`.
#[cfg(any(target_os = "macos", test))]
pub(super) fn resolve_vscode_folder(
    history_roots: &[PathBuf],
    session: &str,
    cwd: Option<&Path>,
) -> Option<PathBuf> {
    if !usable_session_id(session) {
        return None;
    }
    for root in history_roots {
        let Some(bucket) = bucket_for_session(root, session) else {
            continue;
        };
        return match decode_bucket_folder(&bucket) {
            BucketFolder::Folder(folder) => Some(folder),
            // The session ran in a window with no folder.
            BucketFolder::RootlessWindow => None,
            // The bucket says nothing on its own, but a `cwd` archived under the
            // same bucket is still that window's own folder.
            BucketFolder::Unknown => cwd
                .filter(|path| usable_folder(path))
                .filter(|path| sanitize_workspace_id(&path.to_string_lossy()) == bucket)
                .map(Path::to_path_buf),
        };
    }
    // No bucket at all: the plugin is not installed here or changed its layout,
    // so the hook's own cwd is the only candidate left.
    cwd.filter(|path| usable_folder(path))
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch root under `/tmp`, kept deliberately short: the truncation
    /// cases below need the 48-byte base64 prefix of a long path to fall inside
    /// a name rather than on a temp-directory boundary. The caller removes it.
    fn scratch(name: &str) -> PathBuf {
        let path = PathBuf::from("/tmp").join(format!("ast-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    fn vscode_bucket_names_decode_back_to_their_window_folder() {
        // Every `/`, `+` and `=` of the base64 becomes `_`: base64("/") is
        // "Lw==", so a window without a folder is archived under "Lw__".
        assert_eq!(sanitize_workspace_id("/"), "Lw__");
        assert_eq!(decode_bucket_folder("Lw__"), BucketFolder::RootlessWindow);

        let base = scratch("bucket");
        // The umlaut keeps a `/` inside the base64, so the bucket carries inner
        // `_` characters; one extra byte when needed forces the `=` padding.
        let mut name = String::from("ÿÿÿÿ");
        while base.join(&name).to_str().unwrap().len().is_multiple_of(3) {
            name.push('x');
        }
        let folder = base.join(&name);
        std::fs::create_dir_all(&folder).unwrap();
        let raw = base64_encode(folder.to_str().unwrap().as_bytes());
        assert!(raw.contains('/') && raw.ends_with('='));
        let bucket = sanitize_workspace_id(folder.to_str().unwrap());
        assert!(bucket[..bucket.len() - 1].contains('_') && bucket.ends_with('_'));
        assert_eq!(
            decode_bucket_folder(&bucket),
            BucketFolder::Folder(folder.clone())
        );

        // `_` also stands for `+`: a `~` on a base64 group boundary puts a `+`
        // into the name, so the bucket hides a second reading — the round-trip
        // check picks the one that re-encodes to the bucket.
        let mut plus_name = String::from("plus");
        while !base64_encode(
            base.join(format!("{plus_name}~"))
                .to_str()
                .unwrap()
                .as_bytes(),
        )
        .contains('+')
        {
            plus_name.push('x');
        }
        let plus = base.join(format!("{plus_name}~"));
        std::fs::create_dir_all(&plus).unwrap();
        let plus_bucket = sanitize_workspace_id(plus.to_str().unwrap());
        assert!(plus_bucket.contains('_'));
        assert_eq!(
            decode_bucket_folder(&plus_bucket),
            BucketFolder::Folder(plus.clone())
        );

        // A path long enough to be cut at 64 characters is no longer decodable:
        // the 48 bytes that survive fall inside a name, not on a directory.
        let long = base.join("y".repeat(60));
        std::fs::create_dir_all(&long).unwrap();
        let long_bucket = sanitize_workspace_id(long.to_str().unwrap());
        assert_eq!(long_bucket.len(), 64);
        assert_eq!(decode_bucket_folder(&long_bucket), BucketFolder::Unknown);

        // Junk names resolve to nothing instead of panicking.
        for junk in ["", "????", "nope", "Lw__extra", "üü"] {
            assert_eq!(decode_bucket_folder(junk), BucketFolder::Unknown);
        }
        std::fs::remove_dir_all(&base).ok();

        // Real buckets observed in a CodeBuddy VS Code install: the mirrored
        // sanitizer has to reproduce them exactly, and the decoder must never
        // turn one into an unrelated path.
        for (bucket, window) in [
            ("L1VzZXJzL2FwcGxlL0Rlc2t0b3A_", "/Users/apple/Desktop"),
            (
                "L1VzZXJzL2FwcGxlL0RvY3VtZW50cy9BSS1QUk9KRUNUL215LWNvZGUtdGVhbXM_",
                "/Users/apple/Documents/AI-PROJECT/my-code-teams",
            ),
        ] {
            assert_eq!(sanitize_workspace_id(window), bucket);
            match decode_bucket_folder(bucket) {
                BucketFolder::Folder(path) => assert_eq!(path, Path::new(window)),
                // The folder simply is not on this machine.
                BucketFolder::Unknown => {}
                BucketFolder::RootlessWindow => {
                    panic!("{bucket} must not decode to the filesystem root")
                }
            }
        }
    }
    #[test]
    fn vscode_folder_resolution_follows_the_plugin_history_bucket() {
        let base = scratch("resolve");
        let folder = base.join("Work");
        let other = base.join("Other");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        // A decodable bucket wins, even when the hook's cwd says otherwise.
        let bucket = sanitize_workspace_id(folder.to_str().unwrap());
        assert_eq!(
            decode_bucket_folder(&bucket),
            BucketFolder::Folder(folder.clone())
        );
        std::fs::create_dir_all(base.join(&bucket).join("conversations/conv-1")).unwrap();
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "conv-1", Some(&other)),
            Some(folder.clone())
        );
        // A bucket carrying inner `_` characters (base64 `/` and `=`) decodes too.
        let mut umlaut = String::from("ÿÿÿÿ");
        while base.join(&umlaut).to_str().unwrap().len().is_multiple_of(3) {
            umlaut.push('x');
        }
        let special = base.join(&umlaut);
        std::fs::create_dir_all(&special).unwrap();
        let special_bucket = sanitize_workspace_id(special.to_str().unwrap());
        std::fs::create_dir_all(base.join(&special_bucket).join("conversations/conv-6")).unwrap();
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "conv-6", None),
            Some(special.clone())
        );
        // Roots without the session are skipped; the next root is consulted.
        assert_eq!(
            resolve_vscode_folder(&[base.join("nope"), base.clone()], "conv-1", None),
            Some(folder.clone())
        );

        // A `/` bucket is a window without a folder: there is nothing to open.
        std::fs::create_dir_all(base.join("Lw__").join("conversations/conv-2")).unwrap();
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "conv-2", Some(&folder)),
            None
        );
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "conv-2", None),
            None
        );

        // A bucket cut at 64 characters cannot be decoded, but a cwd that
        // sanitizes to that same bucket is its window's own folder.
        let long = base.join("y".repeat(60));
        std::fs::create_dir_all(&long).unwrap();
        let long_bucket = sanitize_workspace_id(long.to_str().unwrap());
        assert_eq!(decode_bucket_folder(&long_bucket), BucketFolder::Unknown);
        std::fs::create_dir_all(base.join(&long_bucket).join("conversations/conv-3")).unwrap();
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "conv-3", Some(&long)),
            Some(long.clone())
        );
        // A cwd from another window is not adopted, and neither is none.
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "conv-3", Some(&folder)),
            None
        );
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "conv-3", None),
            None
        );

        // No bucket anywhere: the cwd is the only candidate, and only when it
        // could be a folder at all.
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "conv-4", Some(&folder)),
            Some(folder.clone())
        );
        for cwd in [None, Some(Path::new("/")), Some(Path::new("relative/path"))] {
            assert_eq!(
                resolve_vscode_folder(std::slice::from_ref(&base), "conv-4", cwd),
                None
            );
        }
        // A session id that could walk out of the history root is never used.
        assert_eq!(
            resolve_vscode_folder(std::slice::from_ref(&base), "../conv-1", Some(&folder)),
            None
        );
        // A missing root is silent: the cwd fallback still applies.
        assert_eq!(
            resolve_vscode_folder(&[base.join("nope")], "conv-1", Some(&folder)),
            Some(folder.clone())
        );
        std::fs::remove_dir_all(&base).ok();
    }
}
