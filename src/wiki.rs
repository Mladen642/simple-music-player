// Albums and genres from Wikipedia. Articles about songs, albums and artists have an
// "infobox", a table in the page's source ("wikitext") like this:
//   {{Infobox song
//   | name   = Numb
//   | artist = [[Linkin Park]]
//   | album  = ''[[Meteora (album)|Meteora]]''
//   | genre  = {{hlist|[[Nu metal]]|[[alternative rock]]}}
//   }}
// The lookup reads those fields: the song's article first, then its album's (the album
// the article names, or the one in the song's tags), then the artist's. What the
// English Wikipedia doesn't have is looked for in the Serbian, Croatian, Bosnian and
// Serbo-Croatian ones, where many bands from the region have articles; genres found
// there are given their English names. Answers are kept in wikipedia.json (the old
// player's answers too), and "nothing found" is asked again after 30 days.
use crate::names::{self, same_name, simplify};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// The other Wikipedias asked when the English one has nothing, in this order
const LOCAL: [&str; 4] = ["sr", "hr", "bs", "sh"];
const ASK_AGAIN: u64 = 30 * 24 * 3600; // seconds
// Answers are marked with this. Ones from before the lookup knew about tagged albums and
// the other Wikipedias (no mark) are asked once more where that could find more.
const VERSION: u64 = 2;

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
        if found.album.is_empty() && found.genres.is_empty() && now().saturating_sub(checked) > ASK_AGAIN {
            return None;
        }
        Some(found)
    }

    // True for an answer from before the lookup could do more, when asking again could
    // find more: no genres at all, or only the artist's while the song's album is known
    pub fn could_be_better(&self, key: &str, album_known: bool) -> bool {
        let Some(entry) = self.data.get("songs").and_then(|songs| songs.get(key)) else { return false };
        if entry.get("v").and_then(Value::as_u64).unwrap_or(1) >= VERSION {
            return false;
        }
        let no_genres = entry.get("genres").and_then(Value::as_array).is_none_or(|list| list.is_empty());
        let from_artist = entry.get("from").and_then(|f| f.get("genres")).and_then(Value::as_str) == Some("artist");
        no_genres || (from_artist && album_known)
    }

    pub fn put(&mut self, key: &str, found: &Found) {
        let songs = self.data.entry("songs").or_insert_with(|| json!({}));
        if let Some(songs) = songs.as_object_mut() {
            let from = if found.genres.is_empty() { json!({}) } else { json!({ "genres": found.genres_from }) };
            songs.insert(key.to_string(), json!({ "album": found.album, "genres": found.genres, "from": from, "checked": now(), "v": VERSION }));
            self.changed = true;
        }
    }

    fn artist_genres(&self, artist: &str) -> Option<Vec<String>> {
        let entry = self.data.get("artists")?.get(&simplify(artist))?;
        let list: Vec<String> = entry.get("genres")?.as_array()?.iter().filter_map(Value::as_str).map(str::to_string).collect();
        // "No genres" is asked again after 30 days (articles get written), and once
        // right away if it's from before the other Wikipedias were asked
        let old = now().saturating_sub(entry.get("checked").and_then(Value::as_u64).unwrap_or(0)) > ASK_AGAIN;
        let before = entry.get("v").and_then(Value::as_u64).unwrap_or(1) < VERSION;
        if list.is_empty() && (old || before) {
            return None;
        }
        Some(list)
    }

    fn put_artist(&mut self, artist: &str, genres: &[String]) {
        let artists = self.data.entry("artists").or_insert_with(|| json!({}));
        if let Some(artists) = artists.as_object_mut() {
            artists.insert(simplify(artist), json!({ "genres": genres, "checked": now(), "v": VERSION }));
            self.changed = true;
        }
    }
}

// ---------- Asking Wikipedia ----------

pub struct Asker {
    agent: ureq::Agent,
    pages: HashMap<String, Option<String>>, // articles read this session, by "language:title" (an album is read once for all its songs)
    last: Option<std::time::Instant>,
    api: String, // where to ask, with {lang} for the Wikipedia ("en", "sr"...)
}

impl Default for Asker {
    fn default() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .user_agent("SimpleMusicPlayer/2.0 (personal music player; albums and genres for a local library)")
            .build();
        Asker { agent, pages: HashMap::new(), last: None, api: "https://{lang}.wikipedia.org/w/api.php".into() }
    }
}

// What an answer says about the titles asked for: the names Wikipedia uses for them
// ("normalized", then "redirects"), so each title asked for finds its page
fn final_title(answer: &Value, asked: &str) -> String {
    let mut title = asked.to_string();
    for step in ["normalized", "redirects", "redirects", "redirects"] {
        let moved = answer.pointer(&format!("/query/{step}")).and_then(Value::as_array).and_then(|list| {
            list.iter().find(|item| item.get("from").and_then(Value::as_str) == Some(title.as_str())).and_then(|item| item.get("to").and_then(Value::as_str))
        });
        if let Some(to) = moved {
            title = to.to_string();
        }
    }
    title
}

impl Asker {
    // lang: which Wikipedia ("en", "sr"...)
    fn ask(&mut self, lang: &str, params: &[(&str, &str)]) -> Result<Value, String> {
        if let Some(last) = self.last {
            let since = last.elapsed();
            if since < Duration::from_millis(250) {
                std::thread::sleep(Duration::from_millis(250) - since); // gently: Wikipedia asks for it
            }
        }
        self.last = Some(std::time::Instant::now());
        let mut request = self.agent.get(&self.api.replace("{lang}", lang)).query("format", "json").query("formatversion", "2");
        for (key, value) in params {
            request = request.query(key, value);
        }
        request.call().map_err(|e| e.to_string())?.into_json::<Value>().map_err(|e| e.to_string())
    }

    fn search(&mut self, lang: &str, query: &str) -> Result<Vec<String>, String> {
        let answer = self.ask(lang, &[("action", "query"), ("list", "search"), ("srsearch", query), ("srlimit", "5")])?;
        Ok(answer.pointer("/query/search").and_then(Value::as_array).map_or(Vec::new(), |list| {
            list.iter().filter_map(|item| item.get("title").and_then(Value::as_str).map(str::to_string)).collect()
        }))
    }

    // Reads several articles in one request (the ones not read yet), so trying a few
    // possible names for an article ("X", "X (band)", "X (musician)") is one question
    fn read(&mut self, lang: &str, titles: &[String]) -> Result<(), String> {
        let mut wanted: Vec<&String> = Vec::new();
        for title in titles {
            let fits = !title.trim().is_empty() && !title.contains('|') && !self.pages.contains_key(&format!("{lang}:{title}"));
            if fits && !wanted.contains(&title) {
                wanted.push(title);
            }
        }
        for group in wanted.chunks(20) {
            let joined = group.iter().map(|title| title.as_str()).collect::<Vec<_>>().join("|");
            let answer = self.ask(lang, &[("action", "query"), ("prop", "revisions"), ("rvprop", "content"), ("rvslots", "main"), ("redirects", "1"), ("titles", &joined)])?;
            let pages = answer.pointer("/query/pages").and_then(Value::as_array).cloned().unwrap_or_default();
            for title in group {
                let name = final_title(&answer, title);
                let text = pages
                    .iter()
                    .find(|page| page.get("title").and_then(Value::as_str) == Some(name.as_str()))
                    .and_then(|page| page.pointer("/revisions/0/slots/main/content"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.pages.insert(format!("{lang}:{title}"), text);
            }
        }
        Ok(())
    }

    // An article's wikitext (following redirects), or None when there's no such article
    fn page(&mut self, lang: &str, title: &str) -> Result<Option<String>, String> {
        self.read(lang, &[title.to_string()])?;
        Ok(self.pages.get(&format!("{lang}:{title}")).cloned().flatten())
    }

    // An infobox of the wanted kind whose artist is this one. sure: the article must
    // name the artist (for albums known only by their name, which other bands use too)
    fn infobox(&mut self, title: &str, kinds: &[&str], artist: &str, sure: bool) -> Result<Option<HashMap<String, String>>, String> {
        let Some(text) = self.page("en", title)? else { return Ok(None) };
        let Some(fields) = infobox(&text, kinds) else { return Ok(None) };
        let credited = fields.get("artist").map(|v| plain(v)).unwrap_or_default();
        if (credited.is_empty() && !sure) || same_name(&credited, artist) {
            Ok(Some(fields))
        } else {
            Ok(None)
        }
    }

    // Looks a song up: album and genres. album: the one the song's tags name, if any
    // (its article has the genres when the song has no article of its own).
    // Err when Wikipedia couldn't be reached.
    pub fn find(&mut self, cache: &std::sync::Mutex<Cache>, artist: &str, title: &str, album: &str) -> Result<Found, String> {
        let (artist, title) = (names::lookup_name(artist), names::lookup_name(title));
        let mut found = Found::default();
        // 1. The song's own article
        let mut album_page = String::new();
        for page in self.search("en", &format!("\"{title}\" {artist} song"))? {
            if !same_name(&page, &title) {
                continue;
            }
            if let Some(fields) = self.infobox(&page, &["song", "single"], &artist, false)? {
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
        // 2. Its album's article: the album the song's article names, or else the one in
        // the song's tags (then the article has to name the artist, to be the right album)
        let from_tags = found.album.is_empty();
        let album = if from_tags { without_kind(&names::lookup_name(album.trim())) } else { found.album.clone() };
        if found.genres.is_empty() && !album.is_empty() {
            let mut pages = vec![album_page, format!("{album} ({artist} album)"), format!("{album} (album)")];
            if from_tags {
                pages.push(album.clone());
            }
            pages.retain(|page| !page.is_empty());
            self.read("en", &pages)?; // (all at once)
            let mut fields = None;
            for page in &pages {
                fields = self.infobox(page, &["album"], &artist, from_tags)?;
                if fields.is_some() {
                    break;
                }
            }
            if fields.is_none() {
                for page in self.search("en", &format!("\"{album}\" {artist} album"))? {
                    if same_name(&page, &album) {
                        if let Some(hit) = self.infobox(&page, &["album"], &artist, from_tags)? {
                            fields = Some(hit);
                            break;
                        }
                    }
                }
            }
            if let Some(value) = fields.as_ref().and_then(|f| f.get("genre")) {
                found.genres = genres(value);
            }
            // ...or the album's article in another Wikipedia
            if found.genres.is_empty() {
                found.genres = self.local_genres(&format!("{album} {artist}"), &album, Some(&artist))?;
            }
            if !found.genres.is_empty() {
                found.genres_from = "album".into();
            }
        }
        // 3. The artist's article
        if found.genres.is_empty() {
            let known = cache.lock().unwrap().artist_genres(&artist);
            let list = match known {
                Some(list) => list,
                None => {
                    let mut pages = vec![artist.clone(), format!("{artist} (band)"), format!("{artist} (musician)"), format!("{artist} (singer)")];
                    self.read("en", &pages)?; // (all four at once)
                    let read_genres = |asker: &mut Asker, pages: &[String]| -> Result<Vec<String>, String> {
                        for page in pages {
                            if let Some(text) = asker.page("en", page)? {
                                if let Some(value) = infobox(&text, &["musical artist", "artist", "band"]).and_then(|f| f.get("genre").cloned()) {
                                    return Ok(genres(&value));
                                }
                            }
                        }
                        Ok(Vec::new())
                    };
                    let mut list = read_genres(self, &pages)?;
                    if list.is_empty() {
                        pages = self.search("en", &format!("{artist} musician OR band OR singer"))?.into_iter().filter(|p| same_name(p, &artist)).take(2).collect();
                        self.read("en", &pages)?;
                        list = read_genres(self, &pages)?;
                    }
                    // ...or the artist's article in another Wikipedia
                    if list.is_empty() {
                        list = self.local_genres(&artist, &artist, None)?;
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

    // Genres from the Serbian, Croatian, Bosnian or Serbo-Croatian Wikipedia: the first
    // article named like `name` (in Latin or Cyrillic letters) that lists genres.
    // mentions: a name the article has to mention (an album's artist).
    fn local_genres(&mut self, search: &str, name: &str, mentions: Option<&str>) -> Result<Vec<String>, String> {
        let simple = |text: &str| names::normalize(&names::to_latin(text)).chars().filter(|c| c.is_alphanumeric()).collect::<String>();
        for lang in LOCAL {
            let pages: Vec<String> = self.search(lang, search)?.into_iter().filter(|page| same_name(&without_kind_any(page), name)).take(2).collect();
            self.read(lang, &pages)?;
            for page in &pages {
                let Some(text) = self.page(lang, page)? else { continue };
                if mentions.is_some_and(|wanted| !simple(&text).contains(&simple(wanted))) {
                    continue;
                }
                let Some(value) = local_field(&text) else { continue };
                let list = self.in_english(lang, &value)?;
                if !list.is_empty() {
                    return Ok(list);
                }
            }
        }
        Ok(Vec::new())
    }

    // A genre field of another Wikipedia with English genre names: a linked genre
    // ("[[Хард рок|хард рок]]") gets the name of its article's English version ("Hard
    // rock"); one that isn't linked is translated by local_genre_name
    fn in_english(&mut self, lang: &str, value: &str) -> Result<Vec<String>, String> {
        static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\[\[([^\[\]|#]+)(?:#[^\[\]|]*)?(?:\|([^\[\]]*))?\]\]").unwrap());
        // What each link shows -> the article it leads to
        let links: Vec<(String, String)> = LINK
            .captures_iter(value)
            .map(|link| (plain(link.get(2).map_or(&link[1], |shown| shown.as_str())).to_lowercase(), link[1].trim().to_string()))
            .filter(|(_, target)| !target.contains(':'))
            .collect();
        let mut english: HashMap<String, String> = HashMap::new();
        let targets: Vec<&str> = links.iter().map(|(_, target)| target.as_str()).filter(|target| !target.contains('|')).take(20).collect();
        if !targets.is_empty() {
            let answer = self.ask(lang, &[("action", "query"), ("prop", "langlinks"), ("lllang", "en"), ("lllimit", "max"), ("redirects", "1"), ("titles", &targets.join("|"))])?;
            let pages = answer.pointer("/query/pages").and_then(Value::as_array).cloned().unwrap_or_default();
            for target in targets {
                let name = final_title(&answer, target);
                let title = pages
                    .iter()
                    .find(|page| page.get("title").and_then(Value::as_str) == Some(name.as_str()))
                    .and_then(|page| page.pointer("/langlinks/0/title"))
                    .and_then(Value::as_str);
                if let Some(title) = title {
                    english.insert(target.to_string(), english_genre_name(title));
                }
            }
        }
        let list: Vec<String> = items(value)
            .into_iter()
            .filter(|item| item.len() < 60)
            .map(|item| {
                let linked = links.iter().find(|(shown, _)| *shown == item.to_lowercase()).and_then(|(_, target)| english.get(target));
                linked.cloned().unwrap_or_else(|| local_genre_name(&item))
            })
            .filter(|name| !name.is_empty())
            .collect();
        Ok(names::clean_genres(&list.join(", ")))
    }
}

// The genre field of an article in another Wikipedia, whatever its infobox is called
// there: the first "| žanr = ..." (or "жанр", "genre"...) of the page
fn local_field(text: &str) -> Option<String> {
    static FIELD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\|\s*(?:žanr|žanrovi|zanr|жанр|жанрови|genre|genres)\s*=").unwrap());
    let rest = &text[FIELD.find(text)?.end()..];
    // The value ends at the next "|" or "}}" that isn't inside a link or a template
    let bytes = rest.as_bytes();
    let (mut depth, mut i) = (0i32, 0);
    while i < bytes.len() {
        let pair = (bytes[i], bytes.get(i + 1).copied().unwrap_or(b' '));
        match pair {
            (b'[', b'[') | (b'{', b'{') => { depth += 1; i += 2; continue; }
            (b']', b']') if depth > 0 => { depth -= 1; i += 2; continue; }
            (b'}', b'}') if depth > 0 => { depth -= 1; i += 2; continue; }
            (b'}', b'}') | (b'|', _) if depth == 0 => break,
            _ => i += 1,
        }
    }
    let value = rest[..i].trim();
    (!value.is_empty()).then(|| value.to_string())
}

// "Rock music" -> "Rock", "Folk (music genre)" -> "Folk": an article's name as a genre's name
fn english_genre_name(title: &str) -> String {
    static EXTRA: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\s*\([^)]*\)\s*$|\s+music$").unwrap());
    let name = EXTRA.replace(title, "");
    EXTRA.replace(&name, "").trim().to_string()
}

// A genre that isn't linked, from Serbian, Croatian or Bosnian ("хард рок", "novi val")
// to its English name ("hard rock", "new wave"). Words not known here stay as they are.
fn local_genre_name(name: &str) -> String {
    const PHRASES: [(&str, &str); 14] = [
        ("novi talas", "new wave"), ("novi val", "new wave"), ("hevi metal", "heavy metal"), ("tvrdi rok", "hard rock"),
        ("narodna muzika", "folk"), ("narodna glazba", "folk"), ("zabavna muzika", "pop"), ("zabavna glazba", "pop"),
        ("elektronska muzika", "electronic"), ("elektronička glazba", "electronic"), ("elektronicka glazba", "electronic"),
        ("klasična muzika", "classical"), ("klasična glazba", "classical"), ("hip hop", "hip hop"),
    ];
    const WORDS: [(&str, &str); 30] = [
        ("rok", "rock"), ("pank", "punk"), ("bluz", "blues"), ("džez", "jazz"), ("dzez", "jazz"), ("rege", "reggae"), ("fank", "funk"),
        ("rep", "rap"), ("disko", "disco"), ("tehno", "techno"), ("indi", "indie"), ("alternativni", "alternative"),
        ("progresivni", "progressive"), ("psihodelični", "psychedelic"), ("garažni", "garage"), ("simfonijski", "symphonic"),
        ("akustični", "acoustic"), ("etno", "ethno"), ("narodna", "folk"), ("zabavna", "pop"), ("elektronska", "electronic"),
        ("elektronička", "electronic"), ("hevi", "heavy"), ("tvrdi", "hard"), ("eksperimentalni", "experimental"), ("gotski", "gothic"),
        ("sintpop", "synth-pop"), ("šlager", "schlager"), ("kantri", "country"), ("pop", "pop"),
    ];
    let mut text = names::to_latin(name).to_lowercase().replace('-', " ");
    for (local, english) in PHRASES {
        text = text.replace(local, english);
    }
    let words: Vec<&str> = text.split_whitespace().filter(|word| !matches!(*word, "muzika" | "glazba" | "music")).map(|word| WORDS.iter().find(|(local, _)| local == &word).map_or(word, |(_, english)| *english)).collect();
    words.join(" ")
}

// "Mrtva priroda (album)", "Мртва природа (албум)" -> the name alone
fn without_kind_any(name: &str) -> String {
    static KIND: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\s*\([^)]*\)\s*$").unwrap());
    KIND.replace(name, "").trim().to_string()
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

    #[test]
    fn other_wikipedias() {
        // (made up, in the shapes these Wikipedias use: any infobox name, a "žanr" or "жанр" field)
        let serbian = "{{Музичар
| име = Рибља чорба
| жанр = [[Хард рок|хард рок]], [[хеви метал]], блуз рок<ref>x</ref>
| године = 1978—
}}
Текст";
        assert_eq!(local_field(serbian).unwrap(), "[[Хард рок|хард рок]], [[хеви метал]], блуз рок<ref>x</ref>");
        let croatian = "{{Infookvir glazbenik | ime = X | žanr = {{hlist|[[rock]]|[[novi val]]}} | djelatno = 1980.}}";
        assert_eq!(local_field(croatian).unwrap(), "{{hlist|[[rock]]|[[novi val]]}}");
        assert_eq!(items(&local_field(croatian).unwrap()), vec!["rock", "novi val"]);
        assert!(local_field("{{Infokutija film | režija = X }}").is_none());
        assert_eq!(local_genre_name("хард рок"), "hard rock");
        assert_eq!(local_genre_name("Novi val"), "new wave");
        assert_eq!(local_genre_name("pop-rok"), "pop rock");
        assert_eq!(local_genre_name("narodna muzika"), "folk");
        assert_eq!(english_genre_name("Rock music"), "Rock");
        assert_eq!(english_genre_name("Heavy metal music"), "Heavy metal");
        assert_eq!(english_genre_name("New wave (music)"), "New wave");
        assert_eq!(without_kind_any("Мртва природа (албум)"), "Мртва природа");
        // Titles asked for find their pages through "normalized" and "redirects"
        let answer: Value = serde_json::from_str(r#"{"query":{"normalized":[{"from":"riblja corba","to":"Riblja corba"}],"redirects":[{"from":"Riblja corba","to":"Riblja čorba"}],"pages":[{"title":"Riblja čorba"}]}}"#).unwrap();
        assert_eq!(final_title(&answer, "riblja corba"), "Riblja čorba");
        assert_eq!(final_title(&answer, "Other"), "Other");
    }

    // The whole lookup against a pretend Wikipedia (a little web server here that answers
    // the way the real one does): a band the English Wikipedia doesn't know, found in the
    // Serbian one, its genres given their English names; then a song with an album in its tags
    #[test]
    fn a_band_only_the_serbian_wikipedia_knows() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = asked.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut line).unwrap();
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                    header.clear();
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let lang = path.trim_start_matches('/').split('/').next().unwrap_or("").to_string();
                let decode = |text: &str| {
                    let bytes = text.replace('+', " ").into_bytes();
                    let (mut out, mut i) = (Vec::new(), 0);
                    while i < bytes.len() {
                        if bytes[i] == b'%' && i + 2 < bytes.len() {
                            out.push(u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16).unwrap());
                            i += 3;
                        } else {
                            out.push(bytes[i]);
                            i += 1;
                        }
                    }
                    String::from_utf8(out).unwrap()
                };
                let params: HashMap<String, String> = path.split_once('?').map_or("", |(_, q)| q).split('&').filter_map(|pair| pair.split_once('=')).map(|(k, v)| (decode(k), decode(v))).collect();
                let get = |key: &str| params.get(key).cloned().unwrap_or_default();
                log.lock().unwrap().push(format!("{lang} {}{}{}", get("list"), get("prop"), if get("titles").is_empty() { String::new() } else { format!(" x{}", get("titles").split('|').count()) }));
                let band = "{{Музичка група\n| име = Мала група\n| жанр = [[Хард рок|хард рок]], [[хеви метал]], блуз рок\n| године = 1980—\n}}\nТекст";
                let album = "{{Албум\n| име = Први албум\n| извођач = [[Мала група]]\n| жанр = [[Нови талас]]\n}}";
                let answer = if get("list") == "search" {
                    let hits: Vec<&str> = match (lang.as_str(), get("srsearch").as_str()) {
                        ("sr", "Mala Grupa") => vec!["Мала група (музичка група)", "Мала група (филм)"],
                        ("sr", "Prvi album Mala Grupa") => vec!["Први албум (албум)"],
                        _ => vec![],
                    };
                    json!({ "query": { "search": hits.iter().map(|title| json!({ "title": title })).collect::<Vec<_>>() } })
                } else if get("prop") == "revisions" {
                    let pages: Vec<Value> = get("titles").split('|').map(|title| match (lang.as_str(), title) {
                        ("sr", "Мала група (музичка група)") => json!({ "title": title, "revisions": [{ "slots": { "main": { "content": band } } }] }),
                        ("sr", "Мала група (филм)") => json!({ "title": title, "revisions": [{ "slots": { "main": { "content": "{{Филм | жанр = [[драма]] }}" } } }] }),
                        ("sr", "Први албум (албум)") => json!({ "title": title, "revisions": [{ "slots": { "main": { "content": album } } }] }),
                        _ => json!({ "title": title, "missing": true }),
                    }).collect();
                    json!({ "query": { "pages": pages } })
                } else {
                    // langlinks: "хеви метал" is the article "Хеви метал", whose English version is "Heavy metal music"
                    json!({ "query": {
                        "normalized": [{ "from": "хеви метал", "to": "Хеви метал" }],
                        "pages": [
                            { "title": "Хард рок", "langlinks": [{ "lang": "en", "title": "Hard rock" }] },
                            { "title": "Хеви метал", "langlinks": [{ "lang": "en", "title": "Heavy metal music" }] },
                            { "title": "Нови талас", "langlinks": [{ "lang": "en", "title": "New wave music" }] },
                        ],
                    } })
                };
                let body = answer.to_string();
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        let cache = std::sync::Mutex::new(Cache { data: Map::new(), changed: false });
        let mut asker = Asker { api: format!("http://127.0.0.1:{port}/{{lang}}/api.php"), ..Default::default() };

        // No album in the tags: the artist's genres, from the Serbian article (not the film's)
        let found = asker.find(&cache, "Mala Grupa", "Pesma", "").unwrap();
        assert_eq!(found, Found { album: String::new(), genres: genres("Hard rock, Heavy metal, blues rock"), genres_from: "artist".into() });
        assert_eq!(cache.lock().unwrap().artist_genres("Mala Grupa").unwrap(), found.genres);
        let so_far = asked.lock().unwrap().clone();
        // English: the song, then the four names an artist's article can have in ONE request, then a search; Serbian: search, the articles, the genres' English names
        assert_eq!(so_far, vec!["en search", "en revisions x4", "en search", "sr search", "sr revisions x2", "sr langlinks x2"]);

        // An album in the tags: its article's genres win over the artist's. (The album stays
        // the tags' own: found.album is only what a song's article names.)
        let found = asker.find(&cache, "Mala Grupa", "Druga pesma", "Prvi album").unwrap();
        assert_eq!(found, Found { album: String::new(), genres: vec!["New wave".to_string()], genres_from: "album".into() });
        let after: Vec<String> = asked.lock().unwrap()[so_far.len()..].to_vec();
        assert_eq!(after, vec!["en search", "en revisions x3", "en search", "sr search", "sr revisions x1", "sr langlinks x1"]);
    }

    #[test]
    fn answers_worth_asking_again() {
        let mut cache = Cache { data: serde_json::from_str(r#"{"songs":{
            "a|none":{"album":"","genres":[],"checked":99999999999},
            "a|artist":{"album":"","genres":["Rock"],"from":{"genres":"artist"},"checked":1},
            "a|song":{"album":"X","genres":["Rock"],"from":{"genres":"song"},"checked":1}
        },"artists":{"old":{"genres":[]},"known":{"genres":["Rock"]}}}"#).unwrap(), changed: false };
        assert!(cache.could_be_better("a|none", false));
        assert!(cache.could_be_better("a|artist", true) && !cache.could_be_better("a|artist", false));
        assert!(!cache.could_be_better("a|song", true) && !cache.could_be_better("a|missing", true));
        assert!(cache.artist_genres("old").is_none(), "asked again: other Wikipedias may know");
        assert_eq!(cache.artist_genres("known").unwrap(), vec!["Rock"]);
        // New answers are marked, and aren't asked again
        cache.put("a|none", &Found::default());
        cache.put_artist("old", &[]);
        assert!(!cache.could_be_better("a|none", true));
        assert_eq!(cache.artist_genres("old").unwrap(), Vec::<String>::new());
    }
}
