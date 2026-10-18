// Albums and genres from Wikipedia. Articles about songs, albums and artists have an
// "infobox", a table in the page's source ("wikitext") like this:
//   {{Infobox song
//   | name   = Numb
//   | artist = [[Linkin Park]]
//   | album  = ''[[Meteora (album)|Meteora]]''
//   | genre  = {{hlist|[[Nu metal]]|[[alternative rock]]}}
//   }}
// The lookup reads those fields: the song's article first, then its album's, then the
// artist's. Answers are kept in wikipedia.json (the old player's answers too), and
// "nothing found" is asked again after 30 days.
use crate::names::{self, same_name, simplify};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API: &str = "https://en.wikipedia.org/w/api.php";

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Found {
    pub album: String,
    pub genres: Vec<String>,
    pub genres_from: String, // "song", "album" or "artist": where the genres were found
}

// The cache, as wikipedia.json keeps it ({"artists": {...}, "songs": {...}})
pub struct Cache {
    data: Map<String, Value>,
    pub changed: bool,
}

pub fn key(artist: &str, title: &str) -> String {
    format!("{}|{}", simplify(&names::lookup_name(artist)), simplify(&names::lookup_name(title)))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Cache {
    pub fn load(data: &Path) -> Cache {
        let value = std::fs::read_to_string(data.join("wikipedia.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        Cache { data: value.and_then(|v| v.as_object().cloned()).unwrap_or_default(), changed: false }
    }

    pub fn save(&mut self, data: &Path) {
        if self.changed {
            let text = serde_json::to_string_pretty(&Value::Object(self.data.clone())).unwrap_or_default();
            let _ = crate::store::write_safely(&data.join("wikipedia.json"), &text);
            self.changed = false;
        }
    }

    // A kept answer that can be used: anything found, or "nothing" from the last 30 days
    pub fn get(&self, key: &str) -> Option<Found> {
        let entry = self.data.get("songs")?.get(key)?;
        let found = Found {
            album: entry.get("album").and_then(Value::as_str).unwrap_or("").to_string(),
            genres: entry.get("genres").and_then(Value::as_array).map_or(Vec::new(), |list| list.iter().filter_map(Value::as_str).map(str::to_string).collect()),
            genres_from: entry.get("from").and_then(|f| f.get("genres")).and_then(Value::as_str).unwrap_or("").to_string(),
        };
        let checked = entry.get("checked").and_then(Value::as_u64).unwrap_or(0);
        if found.album.is_empty() && found.genres.is_empty() && now().saturating_sub(checked) > 30 * 24 * 3600 {
            return None;
        }
        Some(found)
    }

    pub fn put(&mut self, key: &str, found: &Found) {
        let songs = self.data.entry("songs").or_insert_with(|| json!({}));
        if let Some(songs) = songs.as_object_mut() {
            let from = if found.genres.is_empty() { json!({}) } else { json!({ "genres": found.genres_from }) };
            songs.insert(key.to_string(), json!({ "album": found.album, "genres": found.genres, "from": from, "checked": now() }));
            self.changed = true;
        }
    }

    fn artist_genres(&self, artist: &str) -> Option<Vec<String>> {
        let entry = self.data.get("artists")?.get(&simplify(artist))?;
        Some(entry.get("genres")?.as_array()?.iter().filter_map(Value::as_str).map(str::to_string).collect())
    }

    fn put_artist(&mut self, artist: &str, genres: &[String]) {
        let artists = self.data.entry("artists").or_insert_with(|| json!({}));
        if let Some(artists) = artists.as_object_mut() {
            artists.insert(simplify(artist), json!({ "genres": genres, "checked": now() }));
            self.changed = true;
        }
    }
}

// ---------- Asking Wikipedia ----------

pub struct Asker {
    agent: ureq::Agent,
    pages: HashMap<String, Option<String>>, // articles read this session (an album is read once for all its songs)
    last: Option<std::time::Instant>,
}

impl Default for Asker {
    fn default() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .user_agent("SimpleMusicPlayer/2.0 (personal music player; albums and genres for a local library)")
            .build();
        Asker { agent, pages: HashMap::new(), last: None }
    }
}

impl Asker {
    fn ask(&mut self, params: &[(&str, &str)]) -> Result<Value, String> {
        if let Some(last) = self.last {
            let since = last.elapsed();
            if since < Duration::from_millis(250) {
                std::thread::sleep(Duration::from_millis(250) - since); // gently: Wikipedia asks for it
            }
        }
        self.last = Some(std::time::Instant::now());
        let mut request = self.agent.get(API).query("format", "json").query("formatversion", "2");
        for (key, value) in params {
            request = request.query(key, value);
        }
        request.call().map_err(|e| e.to_string())?.into_json::<Value>().map_err(|e| e.to_string())
    }

    fn search(&mut self, query: &str) -> Result<Vec<String>, String> {
        let answer = self.ask(&[("action", "query"), ("list", "search"), ("srsearch", query), ("srlimit", "5")])?;
        Ok(answer.pointer("/query/search").and_then(Value::as_array).map_or(Vec::new(), |list| {
            list.iter().filter_map(|item| item.get("title").and_then(Value::as_str).map(str::to_string)).collect()
        }))
    }

    // An article's wikitext (following redirects), or None when there's no such article
    fn page(&mut self, title: &str) -> Result<Option<String>, String> {
        if let Some(known) = self.pages.get(title) {
            return Ok(known.clone());
        }
        let answer = self.ask(&[("action", "query"), ("prop", "revisions"), ("rvprop", "content"), ("rvslots", "main"), ("redirects", "1"), ("titles", title)])?;
        let text = answer.pointer("/query/pages/0/revisions/0/slots/main/content").and_then(Value::as_str).map(str::to_string);
        self.pages.insert(title.to_string(), text.clone());
        Ok(text)
    }

    // An infobox of the wanted kind whose artist is this one
    fn infobox(&mut self, title: &str, kinds: &[&str], artist: &str) -> Result<Option<HashMap<String, String>>, String> {
        let Some(text) = self.page(title)? else { return Ok(None) };
        let Some(fields) = infobox(&text, kinds) else { return Ok(None) };
        let credited = fields.get("artist").map(|v| plain(v)).unwrap_or_default();
        if credited.is_empty() || same_name(&credited, artist) {
            Ok(Some(fields))
        } else {
            Ok(None)
        }
    }

    // Looks a song up: album and genres. Err when Wikipedia couldn't be reached.
    pub fn find(&mut self, cache: &std::sync::Mutex<Cache>, artist: &str, title: &str) -> Result<Found, String> {
        let (artist, title) = (names::lookup_name(artist), names::lookup_name(title));
        let mut found = Found::default();
        // 1. The song's own article
        let mut album_page = String::new();
        for page in self.search(&format!("\"{title}\" {artist} song"))? {
            if !same_name(&page, &title) {
                continue;
            }
            if let Some(fields) = self.infobox(&page, &["song", "single"], &artist)? {
                if let Some(value) = fields.get("album") {
                    album_page = first_link(value).unwrap_or_default();
                    found.album = items(value).into_iter().next().map(|a| without_kind(&a)).unwrap_or_default();
                }
                if let Some(value) = fields.get("genre") {
                    found.genres = genres(value);
                    if !found.genres.is_empty() {
                        found.genres_from = "song".into();
                    }
                }
                break;
            }
        }
        // 2. Its album's article
        if found.genres.is_empty() && !found.album.is_empty() {
            let page = if album_page.is_empty() { format!("{} ({artist} album)", found.album) } else { album_page };
            let fields = match self.infobox(&page, &["album"], &artist)? {
                Some(fields) => Some(fields),
                None => {
                    let mut hit = None;
                    for page in self.search(&format!("\"{}\" {artist} album", found.album))? {
                        if same_name(&page, &found.album) {
                            if let Some(fields) = self.infobox(&page, &["album"], &artist)? {
                                hit = Some(fields);
                                break;
                            }
                        }
                    }
                    hit
                }
            };
            if let Some(value) = fields.as_ref().and_then(|f| f.get("genre")) {
                found.genres = genres(value);
                if !found.genres.is_empty() {
                    found.genres_from = "album".into();
                }
            }
        }
        // 3. The artist's article
        if found.genres.is_empty() {
            let known = cache.lock().unwrap().artist_genres(&artist);
            let list = match known {
                Some(list) => list,
                None => {
                    let mut list = Vec::new();
                    let mut pages = vec![artist.clone(), format!("{artist} (band)"), format!("{artist} (musician)"), format!("{artist} (singer)")];
                    pages.extend(self.search(&format!("{artist} musician OR band OR singer"))?.into_iter().filter(|p| same_name(p, &artist)).take(2));
                    for page in pages {
                        if let Some(text) = self.page(&page)? {
                            if let Some(value) = infobox(&text, &["musical artist", "artist", "band"]).and_then(|f| f.get("genre").cloned()) {
                                list = genres(&value);
                                break;
                            }
                        }
                    }
                    cache.lock().unwrap().put_artist(&artist, &list);
                    list
                }
            };
            if !list.is_empty() {
                found.genres = list;
                found.genres_from = "artist".into();
            }
        }
        Ok(found)
    }
}

// ---------- Reading wikitext ----------

// The fields of the first "{{Infobox <kind>" in a page: name (lowercase) -> raw value
pub fn infobox(text: &str, kinds: &[&str]) -> Option<HashMap<String, String>> {
    let lower = text.to_lowercase();
    let mut search_from = 0;
    while let Some(found) = lower[search_from..].find("{{infobox") {
        let start = search_from + found;
        let rest = lower[start + 9..].trim_start();
        let kind_ok = kinds.iter().any(|kind| rest.starts_with(kind));
        let end = matching_end(text, start)?;
        if kind_ok {
            let inside = &text[start + 2..end - 2];
            let mut fields = HashMap::new();
            for part in split_top(inside, '|').into_iter().skip(1) {
                if let Some((name, value)) = part.split_once('=') {
                    fields.insert(name.trim().to_lowercase(), value.trim().to_string());
                }
            }
            return Some(fields);
        }
        search_from = start + 9;
    }
    None
}

// Where the "{{" at start closes (just after its "}}")
fn matching_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0i32, start);
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'}' && bytes[i + 1] == b'}' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return Some(i);
            }
        } else {
            i += 1;
        }
    }
    None
}

// Splits at a character, but not inside [[links]] or {{templates}}
fn split_top(text: &str, at: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let (mut links, mut templates) = (0i32, 0i32);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let pair = (chars[i], chars.get(i + 1).copied().unwrap_or(' '));
        match pair {
            ('[', '[') => { links += 1; parts.last_mut().unwrap().push_str("[["); i += 2; continue; }
            (']', ']') => { links -= 1; parts.last_mut().unwrap().push_str("]]"); i += 2; continue; }
            ('{', '{') => { templates += 1; parts.last_mut().unwrap().push_str("{{"); i += 2; continue; }
            ('}', '}') => { templates -= 1; parts.last_mut().unwrap().push_str("}}"); i += 2; continue; }
            _ => {}
        }
        if chars[i] == at && links <= 0 && templates <= 0 {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(chars[i]);
        }
        i += 1;
    }
    parts
}

fn without_noise(value: &str) -> String {
    use std::sync::LazyLock;
    static NOISE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?s)<!--.*?-->|<ref[^>]*/>|<ref[^>]*>.*?</ref>|\{\{(?i:refn|efn|r|sfn|citation needed|cn)[^{}]*\}\}").unwrap());
    NOISE.replace_all(value, "").to_string()
}

// A field as a list: list templates ({{hlist|a|b}}), bullets, <br>, commas
pub fn items(value: &str) -> Vec<String> {
    use std::sync::LazyLock;
    static BREAKS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)<br\s*/?>|\n\s*\*|^\s*\*|·|•|\{\{\s*(·|•|dot|middot|bull|bullet|snd|spnd|ndash|sep|spd)\s*\}\}").unwrap());
    let value = without_noise(value);
    let mut list = Vec::new();
    for piece in expand(&value) {
        for part in BREAKS.split(&piece) {
            for item in split_top(part, ',') {
                let text = plain(&item);
                if !text.is_empty() {
                    list.push(text);
                }
            }
        }
    }
    list
}

// List templates become their items; other templates go (text ones keep their text)
fn expand(part: &str) -> Vec<String> {
    const LISTS: [&str; 14] = ["hlist", "flatlist", "flat list", "plainlist", "plain list", "ubl", "ublist", "unbulleted list", "bulleted list", "blist", "cslist", "flowlist", "unbulleted", "startflatlist"];
    let trimmed = part.trim();
    if let Some(inner) = trimmed.strip_prefix("{{").and_then(|t| t.strip_suffix("}}")) {
        let pieces = split_top(inner, '|');
        let name = pieces[0].trim().to_lowercase();
        if LISTS.contains(&name.as_str()) {
            return pieces[1..].iter().filter(|p| !p.contains('=')).flat_map(|p| expand(p)).collect();
        }
    }
    vec![part.to_string()]
}

// Wikitext as plain text: [[A|B]] -> B, ''italic'' -> italic, templates and tags gone
pub fn plain(value: &str) -> String {
    use std::sync::LazyLock;
    static LINK: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\[\[(?:[^\[\]|]*\|)?([^\[\]]*)\]\]").unwrap());
    static TEMPLATE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\{\{[^{}]*\}\}").unwrap());
    static TAGS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"<[^>]+>|'{2,}|\[\w+://\S+\s?([^\]]*)\]").unwrap());
    let mut text = without_noise(value);
    // Text templates ({{nowrap|...}}) keep their text; the rest go
    loop {
        let next = TEMPLATE.replace_all(&text, |caps: &regex::Captures| {
            let inner = &caps[0][2..caps[0].len() - 2];
            let pieces: Vec<&str> = inner.split('|').collect();
            let name = pieces[0].trim().to_lowercase();
            if ["nowrap", "nobr", "small", "big", "nobold", "noitalic", "sic", "abbr", "not a typo", "proper name", "avoid wrap", "lower"].contains(&name.as_str()) && pieces.len() > 1 {
                pieces[1].to_string()
            } else {
                String::new()
            }
        }).to_string();
        if next == text {
            break;
        }
        text = next;
    }
    let text = LINK.replace_all(&text, "$1");
    let text = TAGS.replace_all(&text, "$1");
    text.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(|c: char| c == ',' || c == '.' || c.is_whitespace()).to_string()
}

fn first_link(value: &str) -> Option<String> {
    let start = value.find("[[")? + 2;
    let end = value[start..].find("]]")? + start;
    Some(value[start..end].split('|').next()?.trim().to_string())
}

// "Meteora (album)" -> "Meteora"
fn without_kind(name: &str) -> String {
    use std::sync::LazyLock;
    static KIND: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\s*\([^)]*\b(album|EP|soundtrack|mixtape)\)\s*$").unwrap());
    KIND.replace(name, "").trim().to_string()
}

fn genres(value: &str) -> Vec<String> {
    const NOT_GENRES: [&str; 9] = ["various", "see below", "see list", "n/a", "none", "unknown", "other", "others", "etc"];
    let list: Vec<String> = items(value).into_iter().filter(|g| !NOT_GENRES.contains(&g.to_lowercase().as_str()) && g.len() < 40).collect();
    names::clean_genres(&list.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_infobox() {
        let page = "Intro {{Short description|x}}\n{{Infobox song\n| name   = Numb\n| artist = [[Linkin Park]]\n| album  = ''[[Meteora (album)|Meteora]]''\n| genre  = {{hlist|[[Nu metal]]|[[alternative rock]]<ref>{{cite web|url=x}}</ref>}}\n| length = {{duration|m=3|s=07}}\n}}\nText";
        let fields = infobox(page, &["song", "single"]).unwrap();
        assert_eq!(plain(&fields["artist"]), "Linkin Park");
        assert_eq!(items(&fields["album"]), vec!["Meteora"]);
        assert_eq!(first_link(&fields["album"]).unwrap(), "Meteora (album)");
        assert_eq!(genres(&fields["genre"]), vec!["Nu metal", "Alternative Rock"]);
        let flat = "{{Infobox album\n| genre = {{flatlist|\n* [[Heavy metal music|Heavy metal]]\n* [[thrash metal]]\n}}\n}}";
        assert_eq!(genres(&infobox(flat, &["album"]).unwrap()["genre"]), vec!["Heavy metal", "Thrash Metal"]);
        assert_eq!(without_kind("Nightmare (Avenged Sevenfold album)"), "Nightmare");
    }
}
