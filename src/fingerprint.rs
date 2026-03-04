//! CurseForge-compatible MurmurHash2 fingerprinting.
//!
//! CurseForge identifies addon files by computing a MurmurHash2 fingerprint
//! over their contents with whitespace bytes stripped. This module provides
//! the hash function, helpers for computing fingerprints, and TOC/XML include
//! chain walking to determine which files belong to an addon's fingerprint.
//!
//! These utilities are not yet wired into the main CLI but are staged for
//! fingerprint-based addon matching (see AGENTS.md § WoWUp reference).

#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MURMURHASH2_M: u32 = 0x5BD1E995;
const MURMURHASH2_R: u32 = 24;
const CURSEFORGE_SEED: u32 = 1;

const WHITESPACE_BYTES: [u8; 4] = [0x09, 0x0A, 0x0D, 0x20];

/// Compute MurmurHash2 (32-bit) for the given data with the specified seed.
///
/// This is the standard Austin Appleby MurmurHash2 algorithm with little-endian
/// block reads, used by CurseForge for addon fingerprinting.
pub fn murmurhash2(data: &[u8], seed: u32) -> u32 {
    let len = data.len();
    let mut h = seed ^ (len as u32);

    let num_blocks = len / 4;
    for i in 0..num_blocks {
        let offset = i * 4;
        let mut k = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);

        k = k.wrapping_mul(MURMURHASH2_M);
        k ^= k >> MURMURHASH2_R;
        k = k.wrapping_mul(MURMURHASH2_M);

        h = h.wrapping_mul(MURMURHASH2_M);
        h ^= k;
    }

    let tail = &data[num_blocks * 4..];
    match tail.len() {
        3 => {
            h ^= (tail[2] as u32) << 16;
            h ^= (tail[1] as u32) << 8;
            h ^= tail[0] as u32;
            h = h.wrapping_mul(MURMURHASH2_M);
        }
        2 => {
            h ^= (tail[1] as u32) << 8;
            h ^= tail[0] as u32;
            h = h.wrapping_mul(MURMURHASH2_M);
        }
        1 => {
            h ^= tail[0] as u32;
            h = h.wrapping_mul(MURMURHASH2_M);
        }
        _ => {}
    }

    h ^= h >> 13;
    h = h.wrapping_mul(MURMURHASH2_M);
    h ^= h >> 15;

    h
}

/// Strip whitespace bytes (tab, newline, carriage return, space) from data.
pub fn strip_whitespace(data: &[u8]) -> Vec<u8> {
    data.iter()
        .copied()
        .filter(|b| !WHITESPACE_BYTES.contains(b))
        .collect()
}

/// Compute a CurseForge-compatible fingerprint for raw file contents.
///
/// Strips whitespace bytes (`\t`, `\n`, `\r`, ` `) then computes
/// MurmurHash2 with seed 1.
pub fn compute_fingerprint(data: &[u8]) -> u32 {
    let stripped = strip_whitespace(data);
    murmurhash2(&stripped, CURSEFORGE_SEED)
}

/// Compute a CurseForge-compatible fingerprint for an addon folder.
///
/// Takes pre-ordered file contents (as determined by the TOC/XML include chain),
/// strips whitespace from each, concatenates the results, and hashes.
pub fn compute_folder_fingerprint(file_contents: &[&[u8]]) -> u32 {
    let mut combined = Vec::new();
    for contents in file_contents {
        combined.extend(strip_whitespace(contents));
    }
    murmurhash2(&combined, CURSEFORGE_SEED)
}

// ---------------------------------------------------------------------------
// TOC/XML include chain walking
// ---------------------------------------------------------------------------

/// Collects the ordered list of files in an addon's TOC/XML include chain.
///
/// CurseForge fingerprints are computed over the files actually loaded by the
/// game client. This function determines that list by:
///
/// 1. Including the TOC file itself
/// 2. Parsing non-comment, non-empty lines as file references
/// 3. For XML files, recursively following `<Script file="...">` and
///    `<Include file="...">` references
///
/// Returns paths in load order. Files that don't exist on disk are silently
/// skipped. Circular references are handled via a visited set.
pub fn collect_toc_file_list(toc_path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut visited = HashSet::new();

    let Some(addon_dir) = toc_path.parent() else {
        return files;
    };

    push_if_new(toc_path, &mut files, &mut visited);

    let Ok(contents) = std::fs::read_to_string(toc_path) else {
        return files;
    };

    collect_toc_references(&contents, addon_dir, &mut files, &mut visited);

    files
}

/// Reads all files in the TOC include chain and computes the folder fingerprint.
pub fn fingerprint_addon_dir(toc_path: &Path) -> std::io::Result<u32> {
    let file_list = collect_toc_file_list(toc_path);
    let mut all_contents = Vec::new();
    for path in &file_list {
        all_contents.push(std::fs::read(path)?);
    }
    let refs: Vec<&[u8]> = all_contents.iter().map(|v| v.as_slice()).collect();
    Ok(compute_folder_fingerprint(&refs))
}

fn collect_toc_references(
    toc_contents: &str,
    base_dir: &Path,
    files: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
) {
    for line in toc_contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let file_path = base_dir.join(normalize_wow_path(line));
        if file_path.is_file() {
            walk_file(&file_path, files, visited);
        }
    }
}

fn walk_file(file_path: &Path, files: &mut Vec<PathBuf>, visited: &mut HashSet<PathBuf>) {
    if !push_if_new(file_path, files, visited) {
        return;
    }

    let is_xml = file_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("xml"));

    if !is_xml {
        return;
    }

    let Ok(contents) = std::fs::read_to_string(file_path) else {
        return;
    };

    let parent = file_path.parent().unwrap_or(file_path);
    for reference in parse_xml_file_references(&contents) {
        let ref_path = parent.join(normalize_wow_path(&reference));
        if ref_path.is_file() {
            walk_file(&ref_path, files, visited);
        }
    }
}

fn push_if_new(path: &Path, files: &mut Vec<PathBuf>, visited: &mut HashSet<PathBuf>) -> bool {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if visited.contains(&key) {
        return false;
    }
    visited.insert(key);
    files.push(path.to_path_buf());
    true
}

/// WoW paths may use backslashes; normalize to forward slashes for cross-platform use.
fn normalize_wow_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Extracts `file="..."` references from `<Script>` and `<Include>` XML tags
/// in document order.
///
/// Uses simple text scanning rather than a full XML parser, which is sufficient
/// for WoW's restricted XML format.
fn parse_xml_file_references(contents: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let lower = contents.to_lowercase();
    let mut pos = 0;

    while pos < lower.len() {
        let script_pos = lower[pos..].find("<script");
        let include_pos = lower[pos..].find("<include");

        let next = match (script_pos, include_pos) {
            (Some(s), Some(i)) => pos + s.min(i),
            (Some(s), None) => pos + s,
            (None, Some(i)) => pos + i,
            (None, None) => break,
        };

        let Some(rel_end) = contents[next..].find('>') else {
            break;
        };
        let tag_str = &contents[next..next + rel_end + 1];
        if let Some(file_val) = extract_file_attribute(tag_str)
            && !file_val.is_empty()
        {
            refs.push(file_val);
        }
        pos = next + rel_end + 1;
    }

    refs
}

/// Extracts the value of a `file="..."` or `file='...'` attribute from a tag string.
fn extract_file_attribute(tag: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let idx = lower.find("file")?;
    let after_file = &tag[idx + 4..];
    let after_eq = after_file.trim_start().strip_prefix('=')?;
    let trimmed = after_eq.trim_start();

    let (quote, rest) = if let Some(stripped) = trimmed.strip_prefix('"') {
        ('"', stripped)
    } else if let Some(stripped) = trimmed.strip_prefix('\'') {
        ('\'', stripped)
    } else {
        return None;
    };

    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn murmurhash2_empty_seed_zero() {
        assert_eq!(murmurhash2(b"", 0), 0);
    }

    #[test]
    fn murmurhash2_empty_seed_one() {
        assert_eq!(murmurhash2(b"", 1), 1540447798);
    }

    #[test]
    fn murmurhash2_single_byte() {
        assert_eq!(murmurhash2(b"a", 1), 626045324);
    }

    #[test]
    fn murmurhash2_two_bytes() {
        assert_eq!(murmurhash2(b"ab", 1), 1692487918);
    }

    #[test]
    fn murmurhash2_three_bytes() {
        assert_eq!(murmurhash2(b"abc", 1), 1621425345);
    }

    #[test]
    fn murmurhash2_one_block() {
        assert_eq!(murmurhash2(b"abcd", 1), 3376380438);
    }

    #[test]
    fn murmurhash2_multi_block_with_tail() {
        assert_eq!(murmurhash2(b"Hello, World!", 1), 613646864);
    }

    #[test]
    fn murmurhash2_longer_input() {
        assert_eq!(murmurhash2(b"local function foo() end", 1), 3542671579);
    }

    #[test]
    fn murmurhash2_seed_zero_nonzero_input() {
        assert_eq!(murmurhash2(b"abcd", 0), 646393889);
    }

    #[test]
    fn murmurhash2_deterministic() {
        let data = b"the quick brown fox";
        let h1 = murmurhash2(data, 1);
        let h2 = murmurhash2(data, 1);
        assert_eq!(h1, h2);
    }

    #[test]
    fn murmurhash2_different_seeds_differ() {
        let data = b"test";
        assert_ne!(murmurhash2(data, 0), murmurhash2(data, 1));
    }

    #[test]
    fn murmurhash2_different_inputs_differ() {
        assert_ne!(murmurhash2(b"foo", 1), murmurhash2(b"bar", 1));
    }

    #[test]
    fn strip_whitespace_removes_all_types() {
        let input = b"a\tb\nc\rd e";
        let stripped = strip_whitespace(input);
        assert_eq!(stripped, b"abcde");
    }

    #[test]
    fn strip_whitespace_preserves_non_whitespace() {
        let input = b"hello";
        assert_eq!(strip_whitespace(input), b"hello");
    }

    #[test]
    fn strip_whitespace_all_whitespace() {
        let input = b" \t\n\r \t\n\r";
        assert_eq!(strip_whitespace(input), b"");
    }

    #[test]
    fn strip_whitespace_empty() {
        assert_eq!(strip_whitespace(b""), b"");
    }

    #[test]
    fn compute_fingerprint_strips_then_hashes() {
        let with_ws = b"local  function\tfoo()\n\r  end";
        let without_ws = b"localfunctionfoo()end";
        assert_eq!(compute_fingerprint(with_ws), murmurhash2(without_ws, 1));
        assert_eq!(compute_fingerprint(with_ws), 3396904022);
    }

    #[test]
    fn compute_fingerprint_no_whitespace_identity() {
        let data = b"nospaces";
        assert_eq!(compute_fingerprint(data), murmurhash2(data, 1));
    }

    #[test]
    fn compute_folder_fingerprint_single_file() {
        let contents = b"local x = 1";
        assert_eq!(
            compute_folder_fingerprint(&[contents]),
            compute_fingerprint(contents)
        );
    }

    #[test]
    fn compute_folder_fingerprint_multiple_files() {
        let file_a = b"local a = 1\n";
        let file_b = b"local b = 2\n";
        let hash = compute_folder_fingerprint(&[file_a, file_b]);

        let mut combined = strip_whitespace(file_a);
        combined.extend(strip_whitespace(file_b));
        assert_eq!(hash, murmurhash2(&combined, 1));
    }

    #[test]
    fn compute_folder_fingerprint_empty() {
        assert_eq!(compute_folder_fingerprint(&[]), murmurhash2(b"", 1));
    }

    #[test]
    fn compute_folder_fingerprint_order_matters() {
        let file_a = b"aaa";
        let file_b = b"bbb";
        let ab = compute_folder_fingerprint(&[file_a, file_b]);
        let ba = compute_folder_fingerprint(&[file_b, file_a]);
        assert_ne!(ab, ba);
    }

    // --- parse_xml_file_references tests ---

    #[test]
    fn xml_parses_script_tag() {
        let xml = r#"<Ui><Script file="Core.lua"/></Ui>"#;
        assert_eq!(parse_xml_file_references(xml), vec!["Core.lua"]);
    }

    #[test]
    fn xml_parses_include_tag() {
        let xml = r#"<Ui><Include file="Sub.xml"/></Ui>"#;
        assert_eq!(parse_xml_file_references(xml), vec!["Sub.xml"]);
    }

    #[test]
    fn xml_parses_multiple_references() {
        let xml = r#"<Ui>
  <Script file="Init.lua"/>
  <Include file="Modules.xml"/>
  <Script file="Core.lua"/>
</Ui>"#;
        assert_eq!(
            parse_xml_file_references(xml),
            vec!["Init.lua", "Modules.xml", "Core.lua"]
        );
    }

    #[test]
    fn xml_handles_case_insensitive_tags() {
        let xml = r#"<Ui><SCRIPT File="a.lua"/><INCLUDE FILE="b.xml"/></Ui>"#;
        assert_eq!(parse_xml_file_references(xml), vec!["a.lua", "b.xml"]);
    }

    #[test]
    fn xml_handles_single_quotes() {
        let xml = r#"<Script file='test.lua'/>"#;
        assert_eq!(parse_xml_file_references(xml), vec!["test.lua"]);
    }

    #[test]
    fn xml_handles_non_self_closing_tags() {
        let xml = r#"<Script file="test.lua"></Script>"#;
        assert_eq!(parse_xml_file_references(xml), vec!["test.lua"]);
    }

    #[test]
    fn xml_handles_extra_whitespace_around_equals() {
        let xml = r#"<Script file = "test.lua" />"#;
        assert_eq!(parse_xml_file_references(xml), vec!["test.lua"]);
    }

    #[test]
    fn xml_skips_empty_file_attribute() {
        let xml = r#"<Script file=""/>"#;
        assert_eq!(parse_xml_file_references(xml), Vec::<String>::new());
    }

    #[test]
    fn xml_no_matches_in_plain_text() {
        let xml = r#"<Ui><Frame name="MyFrame"/></Ui>"#;
        assert_eq!(parse_xml_file_references(xml), Vec::<String>::new());
    }

    #[test]
    fn xml_handles_paths_with_backslashes() {
        let xml = r#"<Include file="Libs\LibStub.xml"/>"#;
        assert_eq!(parse_xml_file_references(xml), vec!["Libs\\LibStub.xml"]);
    }

    // --- extract_file_attribute tests ---

    #[test]
    fn extract_file_double_quotes() {
        assert_eq!(
            extract_file_attribute(r#"<Script file="test.lua"/>"#),
            Some("test.lua".into())
        );
    }

    #[test]
    fn extract_file_single_quotes() {
        assert_eq!(
            extract_file_attribute(r#"<Script file='test.lua'/>"#),
            Some("test.lua".into())
        );
    }

    #[test]
    fn extract_file_no_attribute() {
        assert_eq!(extract_file_attribute(r#"<Frame name="test"/>"#), None);
    }

    // --- collect_toc_file_list tests ---

    fn make_addon(base: &Path, folder: &str) -> PathBuf {
        let dir = base.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn toc_chain_lua_files_only() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "## Interface: 110002\n## Title: MyAddon\nInit.lua\nCore.lua\n";
        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "Init.lua", "local init = true");
        write_file(&addon, "Core.lua", "local core = true");

        let files = collect_toc_file_list(&toc_path);
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(names, vec!["MyAddon.toc", "Init.lua", "Core.lua"]);
    }

    #[test]
    fn toc_chain_skips_comments_and_blanks() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "## Title: MyAddon\n\n# This is a comment\nInit.lua\n\n## Version: 1.0\n";
        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "Init.lua", "x");

        let files = collect_toc_file_list(&toc_path);
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(names, vec!["MyAddon.toc", "Init.lua"]);
    }

    #[test]
    fn toc_chain_skips_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "Init.lua\nMissing.lua\nCore.lua\n";
        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "Init.lua", "x");
        write_file(&addon, "Core.lua", "y");

        let files = collect_toc_file_list(&toc_path);
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(names, vec!["MyAddon.toc", "Init.lua", "Core.lua"]);
    }

    #[test]
    fn toc_chain_follows_xml_includes() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "MyAddon.xml\n";
        let xml = r#"<Ui>
  <Script file="Core.lua"/>
  <Include file="Modules.xml"/>
</Ui>"#;
        let modules_xml = r#"<Ui><Script file="Module1.lua"/></Ui>"#;

        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "MyAddon.xml", xml);
        write_file(&addon, "Core.lua", "core");
        write_file(&addon, "Modules.xml", modules_xml);
        write_file(&addon, "Module1.lua", "mod1");

        let files = collect_toc_file_list(&toc_path);
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "MyAddon.toc",
                "MyAddon.xml",
                "Core.lua",
                "Modules.xml",
                "Module1.lua"
            ]
        );
    }

    #[test]
    fn toc_chain_handles_subdirectory_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "Libs/LibStub.lua\nCore.lua\n";
        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "Libs/LibStub.lua", "libstub");
        write_file(&addon, "Core.lua", "core");

        let files = collect_toc_file_list(&toc_path);
        let names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&addon)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["MyAddon.toc", "Libs/LibStub.lua", "Core.lua"]);
    }

    #[test]
    fn toc_chain_handles_backslash_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "Libs\\LibStub.lua\n";
        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "Libs/LibStub.lua", "libstub");

        let files = collect_toc_file_list(&toc_path);
        assert_eq!(files.len(), 2); // TOC + LibStub.lua
        assert!(files[1].ends_with("LibStub.lua"));
    }

    #[test]
    fn toc_chain_xml_backslash_references() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "Main.xml\n";
        let xml = r#"<Ui><Include file="Sub\Module.xml"/></Ui>"#;
        let sub_xml = r#"<Ui><Script file="Code.lua"/></Ui>"#;

        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "Main.xml", xml);
        write_file(&addon, "Sub/Module.xml", sub_xml);
        write_file(&addon, "Sub/Code.lua", "code");

        let files = collect_toc_file_list(&toc_path);
        assert_eq!(files.len(), 4);
        assert!(files[3].ends_with("Code.lua"));
    }

    #[test]
    fn toc_chain_prevents_circular_references() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "A.xml\n";
        let a_xml = r#"<Ui><Include file="B.xml"/></Ui>"#;
        let b_xml = r#"<Ui><Include file="A.xml"/></Ui>"#;

        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "A.xml", a_xml);
        write_file(&addon, "B.xml", b_xml);

        let files = collect_toc_file_list(&toc_path);
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(names, vec!["MyAddon.toc", "A.xml", "B.xml"]);
    }

    #[test]
    fn toc_chain_deduplicates_shared_files() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "Shared.lua\nA.xml\nB.xml\n";
        let a_xml = r#"<Ui><Script file="Shared.lua"/></Ui>"#;
        let b_xml = r#"<Ui><Script file="Shared.lua"/></Ui>"#;

        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "Shared.lua", "shared");
        write_file(&addon, "A.xml", a_xml);
        write_file(&addon, "B.xml", b_xml);

        let files = collect_toc_file_list(&toc_path);
        let names: Vec<&str> = files
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(names, vec!["MyAddon.toc", "Shared.lua", "A.xml", "B.xml"]);
    }

    #[test]
    fn toc_chain_empty_toc() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");
        let toc_path = write_file(&addon, "MyAddon.toc", "## Title: Empty\n");

        let files = collect_toc_file_list(&toc_path);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn fingerprint_addon_dir_produces_consistent_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let addon = make_addon(tmp.path(), "MyAddon");

        let toc = "Init.lua\nCore.lua\n";
        let toc_path = write_file(&addon, "MyAddon.toc", toc);
        write_file(&addon, "Init.lua", "local init = 1");
        write_file(&addon, "Core.lua", "local core = 2");

        let hash1 = fingerprint_addon_dir(&toc_path).unwrap();
        let hash2 = fingerprint_addon_dir(&toc_path).unwrap();
        assert_eq!(hash1, hash2);

        let toc_bytes = std::fs::read(&toc_path).unwrap();
        let init_bytes = b"local init = 1";
        let core_bytes = b"local core = 2";
        let expected = compute_folder_fingerprint(&[&toc_bytes, init_bytes, core_bytes]);
        assert_eq!(hash1, expected);
    }

    #[test]
    fn fingerprint_addon_dir_file_order_affects_hash() {
        let tmp = tempfile::tempdir().unwrap();

        let addon_a = make_addon(tmp.path(), "A");
        let toc_a = "X.lua\nY.lua\n";
        let toc_a_path = write_file(&addon_a, "A.toc", toc_a);
        write_file(&addon_a, "X.lua", "xxx");
        write_file(&addon_a, "Y.lua", "yyy");

        let addon_b = make_addon(tmp.path(), "B");
        let toc_b = "Y.lua\nX.lua\n";
        let toc_b_path = write_file(&addon_b, "B.toc", toc_b);
        write_file(&addon_b, "X.lua", "xxx");
        write_file(&addon_b, "Y.lua", "yyy");

        let hash_a = fingerprint_addon_dir(&toc_a_path).unwrap();
        let hash_b = fingerprint_addon_dir(&toc_b_path).unwrap();
        assert_ne!(hash_a, hash_b);
    }
}
