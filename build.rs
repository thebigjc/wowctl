use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=CURSEFORGE_API_KEY");

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("embedded_key.rs");

    match env::var("CURSEFORGE_API_KEY") {
        Ok(key) if !key.is_empty() => {
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
