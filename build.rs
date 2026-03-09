use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

const EXPECTED_KEY_LEN: usize = 60;

fn main() {
    println!("cargo:rerun-if-env-changed=CURSEFORGE_API_KEY");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");

    let out_dir = env::var("OUT_DIR").unwrap();

    emit_version();
    emit_embedded_key(Path::new(&out_dir));
}

fn emit_version() {
    let version = git_version().unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=WOWCTL_VERSION={version}");
}

fn git_version() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if version.is_empty() {
        return None;
    }

    Some(version.strip_prefix('v').unwrap_or(&version).to_string())
}

fn emit_embedded_key(out_dir: &Path) {
    let dest = out_dir.join("embedded_key.rs");

    match env::var("CURSEFORGE_API_KEY").map(|k| k.trim().to_string()) {
        Ok(key) if !key.is_empty() => {
            assert_eq!(
                key.len(),
                EXPECTED_KEY_LEN,
                "CURSEFORGE_API_KEY is {} bytes, expected {} — \
                 check for shell variable expansion (use single quotes)",
                key.len(),
                EXPECTED_KEY_LEN,
            );
            println!(
                "cargo:warning=CURSEFORGE_API_KEY is set ({} bytes), embedding obfuscated key",
                key.len()
            );
            let code = format!(
                r#"
pub fn embedded_api_key() -> Option<String> {{
    let s = obfuse::obfuse!("{key}");
    Some(s.as_str().to_owned())
}}
"#
            );
            fs::write(dest, code).unwrap();
        }
        _ => {
            println!("cargo:warning=CURSEFORGE_API_KEY not set, embedded key will be None");
            fs::write(
                dest,
                r#"
pub fn embedded_api_key() -> Option<String> {
    None
}
"#,
            )
            .unwrap();
        }
    }
}
