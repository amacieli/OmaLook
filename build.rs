//! Build script.
//!
//! Embeds the Google OAuth client (a "Desktop app" client; Google documents its secret as not
//! confidential) so a distributed binary can sign users in to Gmail without any setup on their
//! side. Lookup order: env `OMALOOK_GOOGLE_CLIENT_ID` + `OMALOOK_GOOGLE_CLIENT_SECRET`,
//! then `./google_client.json` (git-ignored), then `~/.config/omalook/google_client.json`.
//! With none of them the binary still builds and falls back to that file at run time.

use std::path::PathBuf;

fn client_from_json(path: &PathBuf) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let o = v.get("installed").or_else(|| v.get("web")).unwrap_or(&v);
    let get = |k: &str| o[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    Some((get("client_id")?, get("client_secret")?))
}

fn main() {
    // CXX-Qt build configuration is currently disabled: qmlscene runs in a separate process.
    println!("cargo:rerun-if-changed=src/qt_bridge/mod.rs");

    println!("cargo:rerun-if-env-changed=OMALOOK_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=OMALOOK_GOOGLE_CLIENT_SECRET");
    let project_file = PathBuf::from("google_client.json");
    let home_file = std::env::var("HOME").map(|h| PathBuf::from(h).join(".config/omalook/google_client.json")).ok();
    println!("cargo:rerun-if-changed=google_client.json");
    if let Some(p) = &home_file {
        println!("cargo:rerun-if-changed={}", p.display());
    }

    let from_env = match (std::env::var("OMALOOK_GOOGLE_CLIENT_ID"), std::env::var("OMALOOK_GOOGLE_CLIENT_SECRET")) {
        (Ok(i), Ok(s)) if !i.is_empty() && !s.is_empty() => Some((i, s)),
        _ => None,
    };
    let client = from_env
        .or_else(|| client_from_json(&project_file))
        .or_else(|| home_file.as_ref().and_then(client_from_json));
    match client {
        Some((id, secret)) => {
            println!("cargo:rustc-env=OMALOOK_GOOGLE_CLIENT_ID={}", id);
            println!("cargo:rustc-env=OMALOOK_GOOGLE_CLIENT_SECRET={}", secret);
        }
        None => println!("cargo:warning=no Google OAuth client found: Gmail sign-in will need ~/.config/omalook/google_client.json at run time"),
    }
}
