//! Received-mail attachments: listing (parsed from Graph), the filename rules, and where saved
//! files go. Network calls live on the provider; this module is pure logic plus a little file I/O
//! so it can be tested without a server.

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// One attachment of a received message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub size: i64,
    pub content_type: String,
    /// Part of the HTML body (a signature logo, a picture shown in the text), not a file the
    /// sender attached. Hidden by default.
    pub inline: bool,
    /// "file" (saveable), "item" (an attached message or event, saved as .eml) or
    /// "reference" (a cloud link; nothing to download).
    pub kind: String,
}

impl Attachment {
    /// True when the daemon can fetch bytes for this one.
    pub fn saveable(&self) -> bool {
        self.kind == "file" || self.kind == "item"
    }
}

/// Parse a Graph `/messages/{id}/attachments` response. `body` (the message's HTML, when we
/// have it) lets us call an attachment inline when the HTML refers to it by `cid:`, even though
/// the server did not flag it: Outlook marks only some embedded pictures `isInline`.
pub fn from_graph(list: &serde_json::Value, body: Option<&str>) -> Vec<Attachment> {
    let body_lc = body.map(|b| b.to_ascii_lowercase());
    list["value"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|a| {
            let id = a["id"].as_str()?.to_string();
            let odata = a["@odata.type"].as_str().unwrap_or("");
            let kind = if odata.ends_with("itemAttachment") {
                "item"
            } else if odata.ends_with("referenceAttachment") {
                "reference"
            } else {
                "file"
            };
            let mut name = a["name"].as_str().unwrap_or("").trim().to_string();
            if kind == "item" && !name.to_ascii_lowercase().ends_with(".eml") {
                name.push_str(".eml");
            }
            let cid = a["contentId"].as_str().unwrap_or("").trim().trim_matches(|c| c == '<' || c == '>').to_ascii_lowercase();
            let referenced = !cid.is_empty()
                && body_lc.as_deref().map(|b| b.contains(&format!("cid:{}", cid))).unwrap_or(false);
            Some(Attachment {
                id,
                name,
                size: a["size"].as_i64().unwrap_or(0),
                content_type: a["contentType"].as_str().unwrap_or("").to_string(),
                inline: a["isInline"].as_bool().unwrap_or(false) || referenced,
                kind: kind.to_string(),
            })
        })
        .collect()
}

/// Forward on Graph: `createForward` copies every attachment of the original onto the draft, and
/// the copies get new ids. Given the original's attachment list, the ids the user kept, and the
/// draft's list (all Graph `/attachments` JSON with id, name, size, isInline), return the ids of
/// the draft's attachments to delete. Copies are matched to originals by name and size, as a
/// multiset (two files with the same name and size are two copies). Pictures that are part of the
/// original's HTML (`isInline`) were never offered to the user, so they are always kept.
pub fn unkept_draft_attachments(original: &serde_json::Value, keep: &[String], draft: &serde_json::Value) -> Vec<String> {
    let key = |a: &serde_json::Value| (a["name"].as_str().unwrap_or("").to_string(), a["size"].as_i64().unwrap_or(-1));
    let mut wanted: std::collections::HashMap<(String, i64), usize> = std::collections::HashMap::new();
    for a in original["value"].as_array().into_iter().flatten() {
        let id = a["id"].as_str().unwrap_or("");
        if a["isInline"].as_bool().unwrap_or(false) || keep.iter().any(|k| k == id) {
            *wanted.entry(key(a)).or_default() += 1;
        }
    }
    let mut doomed = Vec::new();
    for a in draft["value"].as_array().into_iter().flatten() {
        let Some(id) = a["id"].as_str() else { continue };
        match wanted.get_mut(&key(a)) {
            Some(n) if *n > 0 => *n -= 1,
            _ => doomed.push(id.to_string()),
        }
    }
    doomed
}

/// Whether a list contains anything the user would call an attachment.
pub fn any_real(list: &[Attachment]) -> bool {
    list.iter().any(|a| !a.inline)
}

/// A name that is safe as one file name inside the download folder: no path separators or
/// control characters (so `../../x` cannot escape), no leading dots (no hidden files), at most
/// ~200 bytes with the extension kept, never empty.
pub fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() {
        return "attachment".to_string();
    }
    const MAX: usize = 200;
    if cleaned.len() <= MAX {
        return cleaned;
    }
    let (stem, ext) = match cleaned.rfind('.') {
        Some(i) if cleaned.len() - i <= 16 => (&cleaned[..i], &cleaned[i..]),
        _ => (cleaned.as_str(), ""),
    };
    let mut cut = MAX.saturating_sub(ext.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &stem[..cut], ext)
}

/// Create `dir/name`, or `dir/name (1).ext`, `(2)`... when that exists. Never overwrites: the
/// file is created atomically (`create_new`), so two saves at once cannot pick the same name.
pub fn create_unique(dir: &Path, name: &str) -> io::Result<(PathBuf, File)> {
    fs::create_dir_all(dir)?;
    let name = sanitize_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 0..10_000u32 {
        let candidate = if n == 0 { name.clone() } else { format!("{} ({}){}", stem, n, ext) };
        let path = dir.join(&candidate);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "too many files with that name"))
}

/// Where saved attachments go: `[attachments] download_dir` in settings.toml (a leading `~/` is
/// expanded), else the desktop's Downloads folder (XDG user dirs), else `~/Downloads`.
pub fn download_dir(settings_path: &Path) -> PathBuf {
    let configured = fs::read_to_string(settings_path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
        .and_then(|v| v.get("attachments")?.get("download_dir")?.as_str().map(str::to_string))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(p) = configured {
        if p == "~" {
            if let Some(h) = dirs::home_dir() { return h; }
        }
        if let Some(rest) = p.strip_prefix("~/") {
            if let Some(h) = dirs::home_dir() { return h.join(rest); }
        }
        return PathBuf::from(p);
    }
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

/// The folder as shown to the user: `~/Downloads` rather than `/home/adam/Downloads`.
pub fn display_dir(dir: &Path) -> String {
    match dirs::home_dir() {
        Some(h) => match dir.strip_prefix(&h) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => dir.display().to_string(),
        },
        None => dir.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("olook-att-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn graph_list_parses_kinds_and_inline_flag() {
        let j = json!({"value": [
            {"@odata.type": "#microsoft.graph.fileAttachment", "id": "A1", "name": "report.pdf", "size": 2048, "contentType": "application/pdf", "isInline": false},
            {"@odata.type": "#microsoft.graph.fileAttachment", "id": "A2", "name": "logo.png", "size": 10, "contentType": "image/png", "isInline": true, "contentId": "logo@x"},
            {"@odata.type": "#microsoft.graph.itemAttachment", "id": "A3", "name": "Fwd: lunch", "size": 99},
            {"@odata.type": "#microsoft.graph.referenceAttachment", "id": "A4", "name": "Shared.docx", "size": 0},
            {"name": "no id"}
        ]});
        let l = from_graph(&j, None);
        assert_eq!(l.len(), 4);
        assert_eq!((l[0].kind.as_str(), l[0].inline, l[0].size), ("file", false, 2048));
        assert!(l[1].inline);
        assert_eq!((l[2].kind.as_str(), l[2].name.as_str()), ("item", "Fwd: lunch.eml"));
        assert!(!l[3].saveable() && l[2].saveable() && l[0].saveable());
        assert!(any_real(&l));
    }

    #[test]
    fn cid_referenced_in_html_counts_as_inline_even_when_unflagged() {
        let j = json!({"value": [
            {"@odata.type": "#microsoft.graph.fileAttachment", "id": "A1", "name": "image001.png", "isInline": false, "contentId": "<IMAGE001@01D>"},
            {"@odata.type": "#microsoft.graph.fileAttachment", "id": "A2", "name": "real.zip", "isInline": false, "contentId": ""}
        ]});
        let html = "<p>hi</p><img src=\"cid:image001@01d\">";
        let with = from_graph(&j, Some(html));
        assert!(with[0].inline && !with[1].inline);
        let without = from_graph(&j, None);
        assert!(!without[0].inline, "no body, no evidence");
    }

    #[test]
    fn only_inline_means_no_real_attachments() {
        let j = json!({"value": [{"id": "A", "name": "sig.png", "isInline": true}]});
        assert!(!any_real(&from_graph(&j, None)));
        assert!(!any_real(&[]));
    }

    #[test]
    fn sanitize_blocks_traversal_and_hidden_files() {
        assert_eq!(sanitize_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_name("a\\b.txt"), "a_b.txt");
        assert_eq!(sanitize_name(".bashrc"), "bashrc");
        assert_eq!(sanitize_name("..."), "attachment");
        assert_eq!(sanitize_name(""), "attachment");
        assert_eq!(sanitize_name("   "), "attachment");
        assert_eq!(sanitize_name("tab\tname\n.pdf"), "tab_name_.pdf");
        assert_eq!(sanitize_name("Résumé 2026.pdf"), "Résumé 2026.pdf");
    }

    #[test]
    fn sanitize_caps_length_but_keeps_extension() {
        let long = format!("{}.pdf", "é".repeat(300));
        let s = sanitize_name(&long);
        assert!(s.len() <= 200 && s.ends_with(".pdf"));
        assert!(s.is_char_boundary(s.len()));
    }

    #[test]
    fn collisions_get_a_numbered_name_and_nothing_is_overwritten() {
        let d = tmp("coll");
        let (p0, mut f0) = create_unique(&d, "a.txt").unwrap();
        f0.write_all(b"first").unwrap();
        let (p1, _) = create_unique(&d, "a.txt").unwrap();
        let (p2, _) = create_unique(&d, "a.txt").unwrap();
        assert_eq!(p0.file_name().unwrap(), "a.txt");
        assert_eq!(p1.file_name().unwrap(), "a (1).txt");
        assert_eq!(p2.file_name().unwrap(), "a (2).txt");
        assert_eq!(fs::read(&p0).unwrap(), b"first");
        let (n, _) = create_unique(&d, "noext").unwrap();
        let (n1, _) = create_unique(&d, "noext").unwrap();
        assert_eq!((n.file_name().unwrap().to_str().unwrap(), n1.file_name().unwrap().to_str().unwrap()), ("noext", "noext (1)"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn creates_missing_download_dir_and_stays_inside_it() {
        let d = tmp("mk").join("nested/Downloads");
        let (p, _) = create_unique(&d, "../../escape.txt").unwrap();
        assert!(p.starts_with(&d));
        let _ = fs::remove_dir_all(d.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn download_dir_honours_settings_override() {
        let d = tmp("cfg");
        let p = d.join("settings.toml");
        fs::write(&p, "[attachments]\ndownload_dir = \"/srv/in\"\n").unwrap();
        assert_eq!(download_dir(&p), PathBuf::from("/srv/in"));
        fs::write(&p, "[attachments]\ndownload_dir = \"~/Stuff\"\n").unwrap();
        assert_eq!(download_dir(&p), dirs::home_dir().unwrap().join("Stuff"));
        fs::write(&p, "[attachments]\ndownload_dir = \"  \"\n").unwrap();
        assert!(download_dir(&p).ends_with("Downloads") || dirs::download_dir().is_some());
        assert!(download_dir(&d.join("missing.toml")).is_absolute());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn display_dir_abbreviates_home() {
        let h = dirs::home_dir().unwrap();
        assert_eq!(display_dir(&h.join("Downloads")), "~/Downloads");
        assert_eq!(display_dir(Path::new("/srv/in")), "/srv/in");
    }

    #[test]
    fn forward_deletes_only_the_copies_the_user_unticked() {
        let att = |id: &str, name: &str, size: i64, inline: bool| serde_json::json!({"id": id, "name": name, "size": size, "isInline": inline});
        let original = serde_json::json!({"value": [att("o1", "a.pdf", 10, false), att("o2", "b.png", 20, false), att("o3", "logo.png", 5, true), att("o4", "a.pdf", 10, false)]});
        // the draft's copies have different ids, in a different order
        let draft = serde_json::json!({"value": [att("d4", "a.pdf", 10, false), att("d2", "b.png", 20, false), att("d3", "logo.png", 5, true), att("d1", "a.pdf", 10, false)]});
        // keep only o1: one of the two identical a.pdf copies goes, b.png goes, the inline logo stays
        let gone = unkept_draft_attachments(&original, &["o1".to_string()], &draft);
        assert_eq!(gone.len(), 2, "{:?}", gone);
        assert!(gone.contains(&"d2".to_string()), "b.png goes");
        assert!(gone.contains(&"d1".to_string()) != gone.contains(&"d4".to_string()), "exactly one of the two identical a.pdf copies goes: {:?}", gone);
        assert!(!gone.contains(&"d3".to_string()), "the inline logo stays");
        // keep everything: nothing is deleted
        assert!(unkept_draft_attachments(&original, &["o1".into(), "o2".into(), "o4".into()], &draft).is_empty());
        // keep nothing: every real copy goes, the inline one stays
        let mut none = unkept_draft_attachments(&original, &[], &draft);
        none.sort();
        assert_eq!(none, vec!["d1".to_string(), "d2".to_string(), "d4".to_string()]);
        // a copy that matches no original is not ours to keep
        let odd = serde_json::json!({"value": [att("x", "unknown.bin", 1, false)]});
        assert_eq!(unkept_draft_attachments(&original, &["o1".into()], &odd), vec!["x".to_string()]);
    }
}
