use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Deserializer};

pub const DEFAULT_URL: &str = "https://www.manifsendirect.fr/data/events.json";

/// Treats an explicit JSON `null` like a missing field.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
pub struct Feed {
    pub generated_at: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub config: Config,
    pub events: Vec<Event>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub date: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub collector: Collector,
}

#[derive(Debug, Default, Deserialize)]
pub struct Collector {
    pub status_last: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Event {
    #[serde(default, deserialize_with = "null_default")]
    pub id: String,
    #[serde(rename = "type", default, deserialize_with = "null_default")]
    pub kind: String,
    pub subtype: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub phase: String,
    #[serde(default, deserialize_with = "null_default")]
    pub title: String,
    pub summary: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub city: String,
    pub district: Option<String>,
    pub place: Option<String>,
    pub starts_at: Option<String>,
    pub expected_end: Option<String>,
    pub updated_at: Option<String>,
    pub crowd: Option<Crowd>,
    pub confirmation: Option<Confirmation>,
    #[serde(default, deserialize_with = "null_default")]
    pub sources: Vec<Source>,
    #[serde(default, deserialize_with = "null_default")]
    pub hidden: bool,
}

#[derive(Debug, Deserialize)]
pub struct Crowd {
    pub value: Option<String>,
    pub by: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Confirmation {
    pub level: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Source {
    pub name: Option<String>,
    pub url: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Now,
    Next,
    Ended,
}

impl Bucket {
    pub const ALL: [Bucket; 3] = [Bucket::Now, Bucket::Next, Bucket::Ended];

    pub fn heading(self) -> &'static str {
        match self {
            Bucket::Now => "EN COURS",
            Bucket::Next => "À VENIR",
            Bucket::Ended => "TERMINÉ",
        }
    }
}

impl Feed {
    /// `generated_at`, or the feed's day when missing.
    pub fn generated(&self) -> &str {
        self.generated_at
            .as_deref()
            .or(self.config.date.as_deref())
            .unwrap_or("")
    }

    /// The feed's day, `YYYY-MM-DD`.
    pub fn today(&self) -> &str {
        self.generated().get(0..10).unwrap_or("")
    }

    pub fn collector_status(&self) -> &str {
        self.config.collector.status_last.as_deref().unwrap_or("?")
    }

    pub fn visible(&self) -> impl Iterator<Item = &Event> {
        self.events.iter().filter(|e| !e.hidden)
    }
}

impl Event {
    pub fn bucket(&self) -> Bucket {
        match self.phase.as_str() {
            "en_cours" | "probablement_termine" => Bucket::Now,
            "a_venir" => Bucket::Next,
            _ => Bucket::Ended,
        }
    }

    pub fn probably_ended(&self) -> bool {
        self.phase == "probablement_termine"
    }

    /// `type/subtype`
    pub fn tag(&self) -> String {
        match self.subtype.as_deref() {
            Some(sub) if !sub.is_empty() => format!("{}/{sub}", self.kind),
            _ => self.kind.clone(),
        }
    }

    /// Start label; ongoing events that started on another day read "en cours".
    pub fn when(&self, today: &str) -> String {
        let starts = self.starts_at.as_deref();
        if self.bucket() == Bucket::Now && starts.and_then(|s| s.get(0..10)) != Some(today) {
            return "en cours".into();
        }
        when_label(starts)
    }

    pub fn end_hint(&self) -> Option<String> {
        let end = hhmm(self.expected_end.as_deref()?)?;
        match self.bucket() {
            Bucket::Now => Some(format!("→ fin estimée ~{end}")),
            Bucket::Next
                if self
                    .starts_at
                    .as_deref()
                    .and_then(hhmm)
                    .is_some_and(|t| t != "00:00") =>
            {
                Some(format!("→ ~{end}"))
            }
            _ => None,
        }
    }

    pub fn location(&self) -> String {
        let mut parts: Vec<&str> = [self.place.as_deref(), self.district.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect();
        parts.dedup();
        parts.join(", ")
    }

    pub fn crowd_label(&self) -> Option<String> {
        let c = self.crowd.as_ref()?;
        let value = c.value.as_deref()?;
        Some(match c.by.as_deref() {
            Some(by) => format!("{value} ({by})"),
            None => value.to_string(),
        })
    }

    pub fn confirmed(&self) -> bool {
        self.confirmation.as_ref().and_then(|c| c.level.as_deref()) == Some("confirme")
    }

    pub fn source_names(&self) -> String {
        let mut names: Vec<&str> = self
            .sources
            .iter()
            .filter_map(|s| s.name.as_deref())
            .collect();
        names.sort_unstable();
        names.dedup();
        names.join(", ")
    }

    /// URL of the most recently published source.
    pub fn latest_url(&self) -> Option<&str> {
        self.sources
            .iter()
            .max_by(|a, b| a.published_at.cmp(&b.published_at))
            .and_then(|s| s.url.as_deref())
    }
}

/// `HH:MM` part of an ISO timestamp.
fn hhmm(ts: &str) -> Option<&str> {
    ts.get(11..16)
}

/// "mar 10/11 13:00", "mar 10/11" when the time is midnight, "heure n.c." when unknown.
pub fn when_label(ts: Option<&str>) -> String {
    let Some(ts) = ts else {
        return "heure n.c.".into();
    };
    let parsed = (|| {
        let y: i32 = ts.get(0..4)?.parse().ok()?;
        let m: u32 = ts.get(5..7)?.parse().ok()?;
        let d: u32 = ts.get(8..10)?.parse().ok()?;
        ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
    })();
    let Some((y, m, d)) = parsed else {
        return ts.to_string();
    };
    const DOW: [&str; 7] = ["dim", "lun", "mar", "mer", "jeu", "ven", "sam"];
    let day = format!("{} {d:02}/{m:02}", DOW[weekday(y, m, d)]);
    match hhmm(ts) {
        Some(t) if t != "00:00" => format!("{day} {t}"),
        _ => day,
    }
}

/// Day of week, 0 = Sunday (Sakamoto).
fn weekday(y: i32, m: u32, d: u32) -> usize {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d as i32).rem_euclid(7) as usize
}

/// Lowercase, strip accents and punctuation, expand "st"/"ste" to "saint"/"sainte".
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => out.push('a'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'î' | 'ï' | 'í' => out.push('i'),
            'ô' | 'ö' | 'ó' => out.push('o'),
            'ù' | 'û' | 'ü' | 'ú' => out.push('u'),
            'ç' => out.push('c'),
            'ÿ' => out.push('y'),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            c if c.is_ascii_alphanumeric() => out.push(c),
            _ => out.push(' '),
        }
    }
    out.split_whitespace()
        .map(|w| match w {
            "st" => "saint",
            "ste" => "sainte",
            w => w,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Default, Clone)]
pub struct CitySummary {
    pub name: String,
    pub now: usize,
    pub next: usize,
    pub ended: usize,
}

/// Visible events grouped by city, busiest first.
pub fn city_summaries(feed: &Feed) -> Vec<CitySummary> {
    let mut map: BTreeMap<&str, CitySummary> = BTreeMap::new();
    for e in feed.visible() {
        let c = map.entry(e.city.as_str()).or_insert_with(|| CitySummary {
            name: e.city.clone(),
            ..Default::default()
        });
        match e.bucket() {
            Bucket::Now => c.now += 1,
            Bucket::Next => c.next += 1,
            Bucket::Ended => c.ended += 1,
        }
    }
    let mut cities: Vec<CitySummary> = map.into_values().collect();
    cities.sort_by(|a, b| {
        b.now
            .cmp(&a.now)
            .then(b.next.cmp(&a.next))
            .then_with(|| fold(&a.name).cmp(&fold(&b.name)))
    });
    cities
}

/// Sort order inside a section: upcoming by start time, the rest by last update, newest first.
pub fn section_order(bucket: Bucket, a: &Event, b: &Event) -> Ordering {
    match bucket {
        Bucket::Next => {
            let key = |e: &Event| e.starts_at.clone().unwrap_or_else(|| "9999".into());
            key(a).cmp(&key(b))
        }
        _ => b.updated_at.cmp(&a.updated_at),
    }
}

// ---------- loading ----------

#[derive(Debug, Clone)]
pub enum Origin {
    Stdin,
    File(PathBuf),
}

pub fn default_cache_file() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("manifsendirect").join("events.json")
}

pub fn parse(bytes: &[u8]) -> Result<Feed, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid feed JSON: {e}"))
}

pub fn read_file(path: &Path) -> Result<Feed, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Age of the local copy, `None` when it cannot be read.
pub fn file_age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
    )
}

/// Missing, empty, or older than `max_age`.
pub fn is_stale(path: &Path, max_age: Duration) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.len() > 0 => file_age(path).is_none_or(|age| age > max_age),
        _ => true,
    }
}

pub fn has_local_copy(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() > 0)
}

/// Downloads the feed and replaces `dest` only once the body parsed,
/// so a failed download never clobbers the local copy.
pub fn download(url: &str, dest: &Path) -> Result<Feed, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .into();
    let bytes = agent
        .get(url)
        .call()
        .and_then(|mut r| r.body_mut().read_to_vec())
        .map_err(|e| format!("{url}: {e}"))?;
    let feed = parse(&bytes)?;

    if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create '{}': {e}", dir.display()))?;
    }
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("events.json");
    let tmp = dest.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = fs::write(&tmp, &bytes).and_then(|()| fs::rename(&tmp, dest));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(format!("cannot write '{}': {e}", dest.display()));
    }
    Ok(feed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_city_names() {
        assert_eq!(fold("Saint-Nazaire"), "saint nazaire");
        assert_eq!(fold("st-nazaire"), "saint nazaire");
        assert_eq!(fold("  ÉVRY–Courcouronnes "), "evry courcouronnes");
        assert_eq!(fold("Ste-Maxime"), "sainte maxime");
        assert_eq!(fold("La Baule"), "la baule");
    }

    #[test]
    fn labels_start_times() {
        assert_eq!(when_label(None), "heure n.c.");
        assert_eq!(
            when_label(Some("2026-11-10T13:00:00+01:00")),
            "mar 10/11 13:00"
        );
        assert_eq!(when_label(Some("2026-11-14T00:00:00+01:00")), "sam 14/11");
        assert_eq!(
            when_label(Some("2024-02-29T08:30:00+01:00")),
            "jeu 29/02 08:30"
        );
        assert_eq!(when_label(Some("garbage")), "garbage");
    }
}
