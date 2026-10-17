//! Contact names for phone numbers, from the vCards KDE Connect's contacts plugin syncs to
//! `~/.local/share/kpeoplevcard/kdeconnect-<device>/*.vcf`. Numbers match on their last ten
//! digits, so "+1 (555) 000-1234" and "5550001234" are the same contact.
//! The directory is re-read only when its modification time changes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

type Names = HashMap<String, String>;
static CACHE: Mutex<Option<(String, SystemTime, Names)>> = Mutex::new(None);

/// The matching key for a phone number: its last ten digits (or all, when shorter).
pub fn key(number: &str) -> String {
    let d: String = number.chars().filter(|c| c.is_ascii_digit()).collect();
    d[d.len().saturating_sub(10)..].to_string()
}

/// Unfold and parse vCard text into (name, numbers) entries.
pub fn parse_vcards(text: &str) -> Vec<(String, Vec<String>)> {
    let text = text.replace("\r\n", "\n").replace("\n ", "");
    let mut out = vec![];
    let (mut fn_, mut n, mut tels): (String, String, Vec<String>) = Default::default();
    for line in text.lines() {
        let up = line.to_ascii_uppercase();
        if up.starts_with("BEGIN:VCARD") { fn_.clear(); n.clear(); tels.clear(); }
        else if up.starts_with("FN:") || up.starts_with("FN;") { fn_ = line.split_once(':').map(|x| x.1.trim().to_string()).unwrap_or_default(); }
        else if up.starts_with("N:") || up.starts_with("N;") {
            // N:Family;Given;Additional;Prefix;Suffix
            let p: Vec<&str> = line.split_once(':').map(|x| x.1).unwrap_or("").split(';').collect();
            n = [p.get(1), p.get(2), p.first()].iter().flatten().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        }
        else if up.starts_with("TEL") { if let Some((_, v)) = line.split_once(':') { tels.push(v.trim().to_string()); } }
        else if up.starts_with("END:VCARD") {
            let name = if !fn_.is_empty() { fn_.clone() } else { n.clone() };
            if !name.is_empty() && !tels.is_empty() { out.push((name, tels.clone())); }
        }
    }
    out
}

fn load(dir: &PathBuf) -> Names {
    let mut m = Names::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x == "vcf").unwrap_or(false) {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    for (name, tels) in parse_vcards(&t) {
                        for t in tels { let k = key(&t); if k.len() >= 7 { m.entry(k).or_insert_with(|| name.clone()); } }
                    }
                }
            }
        }
    }
    m
}

/// Every (name, number) the phone shared, for recipient suggestions. Re-read on each call
/// (a few hundred small files at most; it only runs while the user is typing a recipient).
pub fn contacts(device: &str) -> Vec<(String, String)> {
    let Some(dir) = dirs::data_dir().map(|d| d.join("kpeoplevcard").join(format!("kdeconnect-{}", device))) else { return vec![] };
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x == "vcf").unwrap_or(false) {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    for (name, tels) in parse_vcards(&t) { for t in tels { out.push((name.clone(), t)); } }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Name of the contact owning `number` on `device`, if the phone shared one.
pub fn lookup(device: &str, number: &str) -> Option<String> {
    let dir = dirs::data_dir()?.join("kpeoplevcard").join(format!("kdeconnect-{}", device));
    let mtime = std::fs::metadata(&dir).and_then(|m| m.modified()).ok()?;
    let mut g = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let stale = !matches!(&*g, Some((d, t, _)) if d == device && *t == mtime);
    if stale { *g = Some((device.to_string(), mtime, load(&dir))); }
    g.as_ref().and_then(|(_, _, m)| m.get(&key(number)).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numbers_match_on_last_ten_digits() {
        assert_eq!(key("+1 (555) 000-1234"), key("5550001234"));
        assert_eq!(key("+15550001234"), "5550001234");
    }
    #[test]
    fn parses_name_and_numbers() {
        let v = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Doe;Jane;Q;;\r\nFN:Jane Doe\r\nTEL;CELL:+1-555-000-1234\r\nTEL;WORK:555-000-9999\r\nEND:VCARD\r\nBEGIN:VCARD\r\nN:Roe;Rick;;;\r\nTEL:5551112222\r\nEND:VCARD\r\n";
        let c = parse_vcards(v);
        assert_eq!(c[0], ("Jane Doe".to_string(), vec!["+1-555-000-1234".to_string(), "555-000-9999".to_string()]));
        assert_eq!(c[1].0, "Rick Roe");
    }
}
