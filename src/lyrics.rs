// Lyrics. Where they come from, in this order ("Best match"):
//   1. a .lrc file next to the song,
//   2. the lyrics folder (lyrics you saved),
//   3. lyrics stored inside the song file (its tags),
//   4. LRCLIB, a free lyrics library,
//   5. lrcmux, a free service that asks YouTube Music, Genius, KuGou and LRCLIB at once,
//   6. NetEase Cloud Music, which has timed lyrics for a great many songs,
//   7. Tekstovi.net (songs from Serbia, Croatia, Bosnia...), when the others have nothing.
// 4 to 6 are asked at the same time, and the best answer in that order is used. What was
// found online is remembered in the lyrics-cache folder (and "nothing found" for 30
// days), so a song is only looked up once.
// Timed lyrics win over plain text, even those of a slightly different version of the
// song (the timing buttons line them up). Names are tried several ways, since lyrics
// libraries often spell them differently than the tags do. The menu above the lyrics picks one source instead;
// that choice is kept per song in library.json.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

pub const SOURCES: [(&str, &str); 9] = [
    ("lrc", ".lrc file"),
    ("saved", "Lyrics folder"),
    ("tags", "In the song file"),
    ("lrclib", "LRCLIB"),
    ("ytmusic", "YouTube Music"), // these three are asked through lrcmux
    ("genius", "Genius"),
    ("kugou", "KuGou"),
    ("netease", "NetEase"),
    ("tekstovi", "Tekstovi.net"),
];

pub fn source_name(id: &str) -> String {
    SOURCES.iter().find(|(source, _)| *source == id).map_or(id.to_string(), |(_, name)| name.to_string())
}

#[derive(Clone, Default, Debug)]
pub struct Found {
    pub synced: Option<String>, // .lrc text: "[01:23.45] Some words"
    pub plain: Option<String>,
    pub message: Option<String>, // "Instrumental ♪", "No lyrics found..."
    pub source: String,
    pub source_name: String,
    pub saved: bool,             // already in a file (nothing to save)
    pub other_version: bool,     // plain lyrics of another version of the song
}

impl Found {
    pub fn has_lyrics(&self) -> bool {
        self.synced.is_some() || self.plain.is_some() || self.message.is_some()
    }
}

// What a lookup needs to know about the song
#[derive(Clone)]
pub struct Query {
    pub path: PathBuf,
    pub lyrics_dir: PathBuf,
    pub cache_dir: PathBuf, // what was found online before
    pub file_name: String, // "Artist - Title", for the lyrics folder
    pub artist: String,    // empty when unknown
    pub title: String,
    pub album: String,     // empty unless really known
    pub duration: f32,
}

// The song's own lyrics first, then online (choice: a source picked for this song).
// fresh: ask online again even when an answer is remembered.
pub fn find(query: &Query, choice: Option<&str>, fresh: bool) -> Found {
    // Lyrics on this computer always win: a .lrc next to the song, saved ones or ones
    // inside the file (the one picked for this song first, when it's one of these),
    // never replaced by online ones
    let local = match choice {
        Some("saved") => ["saved", "lrc", "tags"],
        Some("tags") => ["tags", "lrc", "saved"],
        _ => ["lrc", "saved", "tags"],
    };
    for id in local {
        if let Ok(Some(found)) = from_source(query, id) {
            return found;
        }
    }
    if let Some(choice) = choice.filter(|c| !local.contains(c)) {
        let picked = remembered(query, choice, fresh, || match from_source(query, choice) {
            Ok(found) => Online { found: found.filter(Found::has_lyrics), reached_all: true, reached_any: true },
            Err(error) => {
                eprintln!("Lyrics from {choice}: {error}");
                Online::default()
            }
        });
        if let Some(found) = picked.found {
            return found;
        }
    }
    let online = remembered(query, "best", fresh, || best_online(query));
    online.found.unwrap_or_else(|| Found {
        message: Some(if online.reached_any { "No lyrics found for this song.".into() } else { "Couldn't reach the lyrics services. Check your internet connection.".into() }),
        ..Default::default()
    })
}

// What asking online came to
#[derive(Default)]
struct Online {
    found: Option<Found>,
    reached_all: bool, // every source answered: "nothing" can be remembered
    reached_any: bool,
}

// LRCLIB, lrcmux ("best": it picks from all its sources) and NetEase, all asked at the
// same time. The answer used is the one asking them one after another would give:
// the first in that order with timed lyrics (or "Instrumental"), otherwise the best of
// the rest. It's returned as soon as that's settled, without waiting for slower sources.
fn best_online(query: &Query) -> Online {
    const ORDER: [&str; 3] = ["lrclib", "best", "netease"];
    let (send, receive) = std::sync::mpsc::channel();
    for (index, id) in ORDER.into_iter().enumerate() {
        let (query, send) = (query.clone(), send.clone());
        std::thread::spawn(move || {
            let _ = send.send((index, from_source(&query, id)));
        });
    }
    drop(send);
    let wins = |found: &Found| (found.synced.is_some() && !found.other_version) || found.message.is_some();
    let mut answers: [Option<Result<Option<Found>, String>>; 3] = [None, None, None];
    loop {
        // Settled when a winner has no unanswered source before it
        for answer in &answers {
            match answer {
                None => break,
                Some(Ok(Some(found))) if wins(found) => return Online { found: Some(found.clone()), reached_all: true, reached_any: true },
                Some(_) => {}
            }
        }
        match receive.recv() {
            Ok((index, answer)) => {
                if let Err(error) = &answer {
                    eprintln!("Lyrics from {}: {error}", ORDER[index]);
                }
                answers[index] = Some(answer);
            }
            Err(_) => break, // all of them answered
        }
    }
    let mut online = Online {
        found: None,
        reached_all: answers.iter().all(|answer| matches!(answer, Some(Ok(_)))),
        reached_any: answers.iter().any(|answer| matches!(answer, Some(Ok(_)))),
    };
    for found in answers.into_iter().flatten().filter_map(|answer| answer.ok().flatten()) {
        if online.found.as_ref().is_none_or(|best| rank(&found) > rank(best)) {
            online.found = Some(found);
        }
    }
    // Nothing anywhere: songs from Serbia, Croatia, Bosnia... are often only on Tekstovi.net
    if online.found.is_none() {
        match tekstovi(query) {
            Ok(found) => {
                online.reached_any = true;
                online.found = found;
            }
            Err(error) => {
                eprintln!("Lyrics from tekstovi: {error}");
                online.reached_all = false;
            }
        }
    }
    online
}

// ---------- Remembering what was found online ----------

const ASK_AGAIN: u64 = 30 * 24 * 3600; // seconds: "nothing found" and second-best finds are asked again after 30 days

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// The file an answer is kept in. Songs are told apart by artist, title and length (a
// live version has its own lyrics timing); kind: "best", or the source picked for the song.
fn cache_file(query: &Query, kind: &str) -> PathBuf {
    let key = format!("{}|{}|{}", crate::names::simplify(&query.artist), crate::names::simplify(&query.title), query.duration.round() as i64);
    let name = &sha1_smol::Sha1::from(key.as_bytes()).digest().to_string()[..16];
    query.cache_dir.join(format!("{name}-{kind}.json"))
}

// The remembered answer: Some(None) is "nothing found" (asked not long ago)
fn read_cache(file: &Path) -> Option<Option<Found>> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let recent = now().saturating_sub(value.get("checked").and_then(Value::as_u64)?) < ASK_AGAIN;
    if value.get("missing").and_then(Value::as_bool) == Some(true) {
        return recent.then_some(None);
    }
    let found = Found {
        synced: text(&value, "synced"),
        plain: text(&value, "plain"),
        message: text(&value, "message"),
        source: text(&value, "source").unwrap_or_default(),
        source_name: text(&value, "sourceName").unwrap_or_default(),
        saved: false,
        other_version: value.get("otherVersion").and_then(Value::as_bool).unwrap_or(false),
    };
    if !found.has_lyrics() {
        return None;
    }
    // Timed lyrics of this very song are kept for good; anything less may get better
    let best_there_is = found.synced.is_some() && !found.other_version;
    (best_there_is || recent).then_some(Some(found))
}

fn write_cache(file: &Path, query: &Query, found: Option<&Found>) {
    let value = match found {
        Some(found) => serde_json::json!({
            "artist": query.artist, "title": query.title, "checked": now(),
            "source": found.source, "sourceName": found.source_name, "otherVersion": found.other_version,
            "synced": found.synced, "plain": found.plain, "message": found.message,
        }),
        None => serde_json::json!({ "artist": query.artist, "title": query.title, "checked": now(), "missing": true }),
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = crate::store::write_safely(file, &serde_json::to_string_pretty(&value).unwrap_or_default());
}

// The remembered answer, or a new one (which is then remembered: lyrics always, "nothing
// found" only when every source could be reached, so a bad connection isn't mistaken for it)
fn remembered(query: &Query, kind: &str, fresh: bool, ask: impl FnOnce() -> Online) -> Online {
    let file = cache_file(query, kind);
    if !fresh {
        if let Some(found) = read_cache(&file) {
            return Online { found, reached_all: true, reached_any: true };
        }
    }
    let online = ask();
    if online.found.is_some() || online.reached_all {
        write_cache(&file, query, online.found.as_ref());
    }
    online
}

// Which of two finds is better: timed lyrics (even of another version) over plain text
fn rank(found: &Found) -> u8 {
    match (found.synced.is_some(), found.other_version) {
        (true, false) => 4,
        (true, true) => 3,
        (false, false) => 2,
        (false, true) => 1,
    }
}

// The ways to ask for a song, best first: as named, then the first artist alone
// ("A & B", "A feat. B" -> "A") and the title without "(Live)" or "feat. X", then the
// same without accents ("Noćni voz" -> "Nocni voz", Cyrillic in Latin letters), the way
// many libraries spell songs from Serbia, Croatia and Bosnia
pub fn variants(artist: &str, title: &str) -> Vec<(String, String)> {
    static MORE_ARTISTS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)\s*(?:&|,|\+|\bx\b|\band\b|\bvs\.?|\bfeat\.?|\bft\.?|\bfeaturing\b|\bwith\b)\s.*$").unwrap());
    let first_artist = MORE_ARTISTS.replace(artist, "").trim().to_string();
    let simple_title = crate::names::lookup_name(title);
    let mut list = vec![(artist.to_string(), title.to_string()), (artist.to_string(), simple_title.clone())];
    if !first_artist.is_empty() && first_artist != artist {
        list.push((first_artist.clone(), simple_title.clone()));
    }
    let main_artist = if first_artist.is_empty() { artist } else { &first_artist };
    list.push((crate::names::to_latin(main_artist), crate::names::to_latin(&simple_title)));
    list.push((crate::names::without_accents(main_artist), crate::names::without_accents(&simple_title)));
    let mut unique = Vec::new();
    for pair in list {
        if !unique.contains(&pair) {
            unique.push(pair);
        }
    }
    unique
}

// One source: Ok(None) when it has nothing, Err when it couldn't be reached
pub fn from_source(query: &Query, id: &str) -> Result<Option<Found>, String> {
    match id {
        "lrc" => Ok(std::fs::read_to_string(query.path.with_extension("lrc")).ok().map(|text| Found {
            synced: Some(text),
            source: "lrc".into(),
            source_name: source_name("lrc"),
            saved: true,
            ..Default::default()
        })),
        "saved" => Ok(saved(query)),
        "tags" => Ok(embedded(&query.path)),
        "lrclib" => lrclib(query),
        "netease" => netease(query),
        "tekstovi" => tekstovi(query),
        _ => {
            let provider = if id == "best" { "" } else { id };
            match lrcmux(query, provider)? {
                Some(found) => Ok(Some(found)),
                // Once more without accents ("Đorđe Balašević" -> "Dorde Balasevic"), when that's another spelling
                None => match plainly_spelled(query) {
                    Some(plain) => lrcmux(&plain, provider),
                    None => Ok(None),
                },
            }
        }
    }
}

// The same song with its names in Latin letters without accents, if that's different
fn plainly_spelled(query: &Query) -> Option<Query> {
    let (artist, title) = (crate::names::without_accents(&query.artist), crate::names::without_accents(&query.title));
    (artist != query.artist || title != query.title).then(|| Query { artist, title, ..query.clone() })
}

// Lyrics stored inside the song file (the tags of bought or carefully tagged songs
// often have them): timed when they're written like a .lrc file, plain otherwise
fn embedded(path: &Path) -> Option<Found> {
    use lofty::file::TaggedFileExt;
    let options = lofty::config::ParseOptions::new().read_cover_art(false).read_properties(false);
    let file = lofty::probe::Probe::open(path).ok()?.options(options).guess_file_type().ok()?.read().ok()?;
    // (mp3 tags keep them under another name than m4a, FLAC and Ogg tags do)
    use lofty::tag::ItemKey;
    let text = file.tags().iter().find_map(|tag| tag.get_string(ItemKey::Lyrics).or_else(|| tag.get_string(ItemKey::UnsyncLyrics)).map(str::to_string))?;
    let text = text.replace("\r\n", "\n").replace('\r', "\n").trim().to_string();
    if text.is_empty() {
        return None;
    }
    let timed = parse_lrc(&text).iter().filter(|line| !line.text.is_empty()).count() >= 3;
    Some(Found {
        synced: timed.then(|| text.clone()),
        plain: (!timed).then_some(text),
        source: "tags".into(),
        source_name: source_name("tags"),
        ..Default::default()
    })
}

fn saved(query: &Query) -> Option<Found> {
    let read = |extension: &str| std::fs::read_to_string(query.lyrics_dir.join(format!("{}.{extension}", query.file_name))).ok();
    let (synced, plain) = (read("lrc"), read("txt"));
    (synced.is_some() || plain.is_some()).then(|| Found {
        synced,
        plain,
        source: "saved".into(),
        source_name: source_name("saved"),
        saved: true,
        ..Default::default()
    })
}

// One agent per waiting time (6, 8 and 10 seconds are used), each made once, so
// connections to a source are kept open and used again
fn agent(seconds: u64) -> ureq::Agent {
    static AGENTS: LazyLock<std::sync::Mutex<std::collections::HashMap<u64, ureq::Agent>>> = LazyLock::new(Default::default);
    let mut agents = AGENTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    agents
        .entry(seconds)
        .or_insert_with(|| ureq::AgentBuilder::new().timeout(Duration::from_secs(seconds)).user_agent("SimpleMusicPlayer/2.0").build())
        .clone()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|t| !t.trim().is_empty()).map(str::to_string)
}

fn lrclib(query: &Query) -> Result<Option<Found>, String> {
    let base = "https://lrclib.net/api";
    let duration = query.duration.round() as i64;
    let found = |item: &Value| {
        if item.get("instrumental").and_then(Value::as_bool) == Some(true) {
            return Found { message: Some("Instrumental ♪".into()), source: "lrclib".into(), source_name: source_name("lrclib"), ..Default::default() };
        }
        Found { synced: text(item, "syncedLyrics"), plain: text(item, "plainLyrics"), source: "lrclib".into(), source_name: source_name("lrclib"), ..Default::default() }
    };
    let ask = |path: &str, params: &[(&str, String)]| -> Result<Option<Value>, String> {
        let mut request = agent(6).get(&format!("{base}/{path}")).set("Lrclib-Client", "Simple Music Player (desktop)");
        for (key, value) in params {
            request = request.query(key, value);
        }
        match request.call() {
            Ok(response) => response.into_json::<Value>().map(Some).map_err(|e| e.to_string()),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    };
    let has_artist = !query.artist.is_empty();
    if has_artist {
        let mut params = vec![("track_name", query.title.clone()), ("artist_name", query.artist.clone())];
        if !query.album.is_empty() {
            params.push(("album_name", query.album.clone()));
        }
        if duration > 0 {
            params.push(("duration", duration.to_string()));
        }
        let mut exact = ask("get", &params)?;
        if exact.is_none() && !query.album.is_empty() {
            // Album names in tags often differ from LRCLIB's: once more without it. (Any
            // refusal here just means "not this way": the searches below come next.)
            params.retain(|(key, _)| *key != "album_name");
            exact = ask("get", &params).ok().flatten();
        }
        if let Some(item) = exact {
            let result = found(&item);
            if result.has_lyrics() {
                return Ok(Some(result));
            }
        }
    }
    // Searches: each way of naming the song, then free text ("artist title")
    let mut searches: Vec<Vec<(&str, String)>> = Vec::new();
    if has_artist {
        for (artist, title) in variants(&query.artist, &query.title) {
            searches.push(vec![("track_name", title), ("artist_name", artist)]);
        }
        searches.push(vec![("q", format!("{} {}", query.artist, crate::names::lookup_name(&query.title)))]);
    } else {
        searches.push(vec![("q", crate::names::lookup_name(&query.title))]);
    }
    let mut candidates: Vec<Value> = Vec::new();
    for params in searches {
        let results = ask("search", &params)?.and_then(|v| v.as_array().cloned()).unwrap_or_default();
        for item in results {
            if !candidates.iter().any(|other| other.get("id") == item.get("id")) {
                candidates.push(item);
            }
        }
        let has_timed = candidates.iter().any(|item| text(item, "syncedLyrics").is_some() && within(item, duration, 5.0));
        if has_timed {
            break; // found what's wanted: no need to ask more ways
        }
    }
    // Only songs by the same artist (free-text searches can find anyone)
    if has_artist {
        let artists: Vec<String> = variants(&query.artist, &query.title).into_iter().map(|(artist, _)| artist).collect();
        candidates.retain(|item| artists.iter().any(|artist| crate::names::same_name(&text(item, "artistName").unwrap_or_default(), artist)));
    }
    let timed = |tolerance: f32| candidates.iter().find(|item| text(item, "syncedLyrics").is_some() && within(item, duration, tolerance));
    if let Some(item) = timed(5.0) {
        return Ok(Some(found(item)));
    }
    if let Some(item) = timed(15.0) {
        return Ok(Some(Found { other_version: true, ..found(item) })); // a little longer or shorter: Timing lines it up
    }
    if let Some(item) = candidates.iter().find(|item| text(item, "plainLyrics").is_some() && within(item, duration, 5.0)) {
        return Ok(Some(found(item)));
    }
    let other = if has_artist { candidates.iter().find_map(|item| text(item, "plainLyrics")) } else { None };
    Ok(other.map(|plain| Found { plain: Some(plain), source: "lrclib".into(), source_name: source_name("lrclib"), other_version: true, ..Default::default() }))
}

// True when a found song's length is close to this one's (seconds)
fn within(item: &Value, duration: i64, tolerance: f32) -> bool {
    duration == 0 || item.get("duration").and_then(Value::as_f64).is_some_and(|d| (d - duration as f64).abs() <= tolerance as f64)
}

// NetEase Cloud Music: search, then the lyrics of the best match. (Its public web API,
// used without an account; it's unofficial, so it may change some day.)
fn netease(query: &Query) -> Result<Option<Found>, String> {
    if query.artist.is_empty() {
        return Ok(None);
    }
    let get = |url: &str, params: &[(&str, &str)]| -> Result<Value, String> {
        let mut request = agent(8).get(url).set("Referer", "https://music.163.com/").set("Cookie", "os=pc; appver=2.9.7");
        for (key, value) in params {
            request = request.query(key, value);
        }
        request.call().map_err(|e| e.to_string())?.into_json::<Value>().map_err(|e| e.to_string())
    };
    let duration = query.duration;
    for (artist, title) in variants(&query.artist, &query.title).into_iter().skip(1).take(2) {
        let term = format!("{artist} {title}");
        let answer = get("https://music.163.com/api/search/pc", &[("s", &term), ("type", "1"), ("limit", "10"), ("offset", "0")])?;
        let songs = answer.get("result").and_then(|r| r.get("songs")).and_then(Value::as_array).cloned().unwrap_or_default();
        let length = |song: &Value| song.get("duration").or_else(|| song.get("dt")).and_then(Value::as_f64).map(|ms| ms as f32 / 1000.0);
        let by_artist = |song: &&Value| {
            song.get("artists").or_else(|| song.get("ar")).and_then(Value::as_array).is_some_and(|artists| {
                artists.iter().any(|a| crate::names::same_name(&text(a, "name").unwrap_or_default(), &artist))
            }) && crate::names::same_name(&text(song, "name").unwrap_or_default(), &title)
        };
        let mut matches: Vec<&Value> = songs.iter().filter(by_artist).collect();
        matches.sort_by_key(|song| length(song).map_or(999, |l| (l - duration).abs().round() as i64));
        let Some(song) = matches.first() else { continue };
        let Some(id) = song.get("id").and_then(Value::as_i64) else { continue };
        let lyrics = get("https://music.163.com/api/song/lyric", &[("id", &id.to_string()), ("lv", "1")])?;
        let lrc = lyrics.get("lrc").and_then(|l| l.get("lyric")).and_then(Value::as_str).unwrap_or("");
        let base = Found { source: "netease".into(), source_name: source_name("netease"), ..Default::default() };
        if lyrics.get("nolyric").and_then(Value::as_bool) == Some(true) || lrc.contains("纯音乐") {
            return Ok(Some(Found { message: Some("Instrumental ♪".into()), ..base }));
        }
        let synced = without_credits(lrc);
        if parse_lrc(&synced).iter().filter(|line| !line.text.is_empty()).count() < 3 {
            continue;
        }
        let other_version = duration > 0.0 && length(song).is_some_and(|l| (l - duration).abs() > 5.0);
        if other_version && length(song).is_some_and(|l| (l - duration).abs() > 15.0) {
            continue;
        }
        return Ok(Some(Found { synced: Some(synced), other_version, ..base }));
    }
    Ok(None)
}

// NetEase starts lyrics with credits ("作词 : Someone"): those lines go
fn without_credits(lrc: &str) -> String {
    static CREDIT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^\s*(\[[^\]]*\])+\s*(作词|作曲|编曲|制作人|混音|母带|和声|吉他|贝斯|鼓|出品|发行|监制|录音)\s*[:：]").unwrap());
    lrc.lines().filter(|line| !CREDIT.is_match(line)).collect::<Vec<_>>().join("\n")
}

fn lrcmux(query: &Query, provider: &str) -> Result<Option<Found>, String> {
    if query.artist.is_empty() {
        return Ok(None);
    }
    let duration = query.duration.round() as i64;
    let mut request = agent(10)
        .get("https://api.lrcmux.dev/get")
        .query("artist", &query.artist)
        .query("title", &crate::names::lookup_name(&query.title))
        .query("level", "line")
        .query("format", "json");
    if !query.album.is_empty() {
        request = request.query("album", &query.album);
    }
    if duration > 0 {
        request = request.query("duration", &duration.to_string());
    }
    if !provider.is_empty() {
        request = request.query("sources", provider);
    }
    let answer: Value = match request.call() {
        Ok(response) => response.into_json().map_err(|e| e.to_string())?,
        // Busy (429) or broken (5xx) isn't "no lyrics": it's asked again another time
        Err(ureq::Error::Status(code, _)) if code == 429 || code >= 500 => return Err(format!("lrcmux is busy ({code})")),
        Err(ureq::Error::Status(_, _)) => return Ok(None), // not found
        Err(error) => return Err(error.to_string()),
    };
    let meta = answer.get("meta").cloned().unwrap_or(Value::Null);
    let source = meta.get("source").cloned().unwrap_or(Value::Null);
    let id = if provider.is_empty() { text(&source, "id").unwrap_or_else(|| "lrcmux".into()) } else { provider.to_string() };
    let name = text(&source, "name").unwrap_or_else(|| source_name(&id));
    let base = Found { source: id, source_name: name, ..Default::default() };
    if meta.get("instrumental").and_then(Value::as_bool) == Some(true) {
        return Ok(Some(Found { message: Some("Instrumental ♪".into()), ..base }));
    }
    let lines = answer.get("lines").and_then(Value::as_array).cloned().unwrap_or_default();
    if lines.is_empty() {
        return Ok(None);
    }
    let plain = lines.iter().map(|line| text(line, "text").unwrap_or_default()).collect::<Vec<_>>().join("\n");
    let track_duration = answer.get("track").and_then(|t| t.get("duration")).and_then(Value::as_f64);
    let same_length = duration == 0 || track_duration.is_none_or(|d| (d - duration as f64).abs() <= 5.0);
    let level = meta.get("level").and_then(Value::as_str).unwrap_or("none");
    if level != "none" && same_length {
        let synced = lines
            .iter()
            .map(|line| format!("[{}] {}", lrc_time(line.get("start").and_then(Value::as_f64).unwrap_or(0.0)), text(line, "text").unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(Some(Found { synced: Some(synced), plain: Some(plain), ..base }));
    }
    Ok(Some(Found { plain: Some(plain), other_version: !same_length, ..base }))
}

// ---------- Tekstovi.net ----------

// Tekstovi.net, a large collection of lyrics of songs from Serbia, Croatia, Bosnia and
// their neighbours (plain text, no timing). It has no lookup service, so its pages are
// read: the list of artists under the artist's first letter, the artist's list of
// songs, then the song's page.
fn tekstovi(query: &Query) -> Result<Option<Found>, String> {
    tekstovi_at("https://tekstovi.net", query)
}

fn tekstovi_at(base: &str, query: &Query) -> Result<Option<Found>, String> {
    if query.artist.is_empty() {
        return Ok(None);
    }
    let simple = |text: &str| crate::names::simplify(&crate::names::to_latin(text));
    let ways = variants(&query.artist, &query.title);
    let artists: Vec<String> = ways.iter().map(|(artist, _)| simple(artist)).collect();
    let titles: Vec<String> = ways.iter().map(|(_, title)| simple(title)).collect();

    // The artist, in the list of its first letter ("0-9" is the page named "@")
    // Č, Ć, Đ, Š and Ž have lists of their own: a name typed without accents ("Zeljko")
    // may be under the accented letter
    let latin = crate::names::to_latin(&query.artist);
    let Some(first) = latin.chars().find(|c| c.is_alphanumeric()).and_then(|c| c.to_uppercase().next()) else { return Ok(None) };
    let mut letters = vec![if first.is_ascii_digit() { '@' } else { first }];
    letters.extend(match first {
        'C' => vec!['Č', 'Ć'],
        'D' => vec!['Đ'],
        'S' => vec!['Š'],
        'Z' => vec!['Ž'],
        _ => Vec::new(),
    });
    let mut artist_id = None;
    for letter in letters {
        let index = tekstovi_page(&format!("{base}/2,{letter},0.html"), true)?;
        artist_id = tekstovi_links(&index).into_iter().find(|(_, song, name)| song == "0" && artists.contains(&simple(name))).map(|(id, _, _)| id);
        if artist_id.is_some() {
            break;
        }
    }
    let Some(artist_id) = artist_id else { return Ok(None) };
    // The song, in the artist's list
    let songs = tekstovi_page(&format!("{base}/2,{artist_id},0.html"), false)?;
    let Some((_, song_id, _)) = tekstovi_links(&songs).into_iter().find(|(artist, song, name)| *artist == artist_id && song != "0" && titles.contains(&simple(name))) else {
        return Ok(None);
    };
    let page = tekstovi_page(&format!("{base}/2,{artist_id},{song_id}.html"), false)?;
    Ok(tekstovi_lyrics(&page).map(|plain| Found { plain: Some(plain), source: "tekstovi".into(), source_name: source_name("tekstovi"), ..Default::default() }))
}

// A page of the site. The lists of artists are kept while the player runs (one per
// letter serves every song of every artist under it).
fn tekstovi_page(url: &str, keep: bool) -> Result<String, String> {
    static KEPT: LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = LazyLock::new(Default::default);
    if let Some(page) = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(url) {
        return Ok(page.clone());
    }
    let page = match agent(8).get(url).call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(404, _)) => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    if keep {
        KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(url.to_string(), page.clone());
    }
    Ok(page)
}

// The site's links to artists ("2,293,0.html") and songs ("2,293,5512.html"):
// (artist number, song number, the link's text without a count like "(66)" at its end)
fn tekstovi_links(page: &str) -> Vec<(String, String, String)> {
    static LINK: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r#"(?is)<a\s[^>]*href\s*=\s*["'][^"']*?\b2,(\d+),(\d+)\.html["'][^>]*>(.*?)</a>"#).unwrap());
    static COUNT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\s*\(\d+\)\s*$").unwrap());
    LINK.captures_iter(page)
        .map(|link| (link[1].to_string(), link[2].to_string(), COUNT.replace(html_text(&link[3]).trim(), "").to_string()))
        .collect()
}

// The lyrics on a song's page: the paragraphs marked as lyrics, or else the stretch of
// the page with the most line breaks (a song's lines), if it looks like a song
fn tekstovi_lyrics(page: &str) -> Option<String> {
    static MARKED: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"(?is)<(p|div)\s[^>]*class\s*=\s*["'][^"']*\blyric\b[^"']*["'][^>]*>(.*?)</(p|div)>"#).unwrap());
    static BLOCK: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?is)<script.*?</script>|<style.*?</style>|</?(?:p|div|section|article|aside|header|footer|nav|form|table|tr|td|ul|ol|li|h[1-6]|textarea|button)\b[^>]*>").unwrap()
    });
    let marked: Vec<String> = MARKED.captures_iter(page).map(|block| html_text(&block[2])).filter(|text| !text.is_empty()).collect();
    let text = if marked.is_empty() {
        let breaks = |piece: &&str| piece.to_lowercase().matches("<br").count();
        let longest = BLOCK.split(page).max_by_key(breaks)?;
        if breaks(&longest) < 6 {
            return None;
        }
        html_text(longest)
    } else {
        marked.join("\n\n")
    };
    (text.lines().filter(|line| !line.trim().is_empty()).count() >= 4).then_some(text)
}

// A piece of a web page as plain text: line breaks kept, tags gone
fn html_text(html: &str) -> String {
    static BREAK: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\s*<br\s*/?>\s*").unwrap());
    static TAG: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?s)<[^>]*>").unwrap());
    static GAPS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\n{3,}").unwrap());
    let text = html.replace(['\r', '\n'], " ");
    let text = BREAK.replace_all(&text, "\n");
    let text = TAG.replace_all(&text, "");
    static NUMBERED: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"&#(\d+);").unwrap());
    let text = NUMBERED.replace_all(&text, |code: &regex::Captures| code[1].parse::<u32>().ok().and_then(char::from_u32).map_or(String::new(), String::from));
    let text = text.replace("&nbsp;", " ").replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
    let text = text.lines().map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n");
    GAPS.replace_all(text.trim(), "\n\n").to_string()
}

// ---------- Timed lyrics ----------

pub struct Line {
    pub time: f32,
    pub text: String,
}

// "[01:23.45] Some words" -> 83.45 s, "Some words". A line can have several times (a
// repeated chorus); info lines like "[ar:Artist]" and word times "<01:23.45>" go away.
pub fn parse_lrc(text: &str) -> Vec<Line> {
    use std::sync::LazyLock;
    static STAMP: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\[(\d+):(\d+(?:\.\d+)?)\]").unwrap());
    static TAGS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\[[^\]]*\]|<\d+:\d+(?:\.\d+)?>").unwrap());
    let mut lines = Vec::new();
    for row in text.lines() {
        let words = TAGS.replace_all(row, "").trim().to_string();
        for stamp in STAMP.captures_iter(row) {
            let time = stamp[1].parse::<f32>().unwrap_or(0.0) * 60.0 + stamp[2].parse::<f32>().unwrap_or(0.0);
            lines.push(Line { time, text: words.clone() });
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

// Milliseconds as an .lrc time: "01:23.45" (whole hundredths, so 59.996 s can't become "00:60.00")
pub fn lrc_time(milliseconds: f64) -> String {
    let hundredths = (milliseconds / 10.0).round().max(0.0) as u64;
    format!("{:02}:{:05.2}", hundredths / 6000, (hundredths % 6000) as f64 / 100.0)
}

// Moves every time in .lrc text ("[01:23.45]" and word times "<01:23.45>") by some
// seconds, never below zero. Info lines like "[ar:Artist]" stay as they are.
pub fn shift_lrc(text: &str, seconds: f32) -> String {
    use std::sync::LazyLock;
    static TIME: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"([\[<])(\d+):(\d+(?:\.\d+)?)([\]>])").unwrap());
    TIME.replace_all(text, |caps: &regex::Captures| {
        let at = caps[2].parse::<f64>().unwrap_or(0.0) * 60.0 + caps[3].parse::<f64>().unwrap_or(0.0) + seconds as f64;
        format!("{}{}{}", &caps[1], lrc_time(at * 1000.0), &caps[4])
    })
    .to_string()
}

// ---------- Saving ----------

// Characters file names can't have: "AC/DC" -> "AC-DC", "Title: Part 2" -> "Title - Part 2"
pub fn safe_file_name(name: &str) -> String {
    let name: String = name
        .replace(['/', '\\', '|'], "-")
        .replace(':', " -")
        .replace('"', "'")
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | '*' | '?') && !c.is_control())
        .collect();
    name.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches(['.', ' ']).to_string()
}

// Saved lyrics are named after the song: "Artist - Title"
pub fn file_name(artist: &str, title: &str) -> String {
    let name = if artist.is_empty() { title.to_string() } else { format!("{artist} - {title}") };
    let name = safe_file_name(&name);
    if name.is_empty() { "lyrics".into() } else { name }
}

// A .lrc file: a few lines about the song, then the timed lyrics (the source's own
// info lines left out)
pub fn lrc_file_text(artist: &str, title: &str, album: &str, duration: f32, synced: &str) -> String {
    let mut lines = vec![format!("[ar:{artist}]"), format!("[ti:{title}]")];
    if !album.is_empty() {
        lines.push(format!("[al:{album}]"));
    }
    if duration > 0.0 {
        lines.push(format!("[length:{}]", crate::app::format_time(duration)));
    }
    static INFO: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^\[[a-z]+:").unwrap());
    lines.extend(synced.lines().filter(|row| !INFO.is_match(row)).map(str::to_string));
    lines.join("\n") + "\n"
}

// Timed lyrics as .lrc, plain ones as .txt; the other kind saved earlier is replaced
pub fn save(dir: &Path, name: &str, text: &str, synced: bool) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (extension, other) = if synced { ("lrc", "txt") } else { ("txt", "lrc") };
    let file = format!("{name}.{extension}");
    std::fs::write(dir.join(&file), text).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(dir.join(format!("{name}.{other}")));
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing() {
        let text = "[ar:X]\n[00:01.50] Hello <00:01.60>w\n[00:00.20][01:59.99] Chorus\n";
        assert_eq!(shift_lrc(text, 0.5), "[ar:X]\n[00:02.00] Hello <00:02.10>w\n[00:00.70][02:00.49] Chorus\n");
        assert_eq!(shift_lrc(text, -1.0), "[ar:X]\n[00:00.50] Hello <00:00.60>w\n[00:00.00][01:58.99] Chorus\n");
        assert_eq!(lrc_time(59996.0), "01:00.00");
        let lines = parse_lrc(text);
        assert_eq!(lines.len(), 3);
        assert_eq!((lines[0].time, lines[0].text.as_str()), (0.2, "Chorus"));
        assert_eq!(lines[1].text, "Hello w");
        assert_eq!(file_name("AC/DC", "Title: Part 2?"), "AC-DC - Title - Part 2");
        let ways = variants("Linkin Park & Jay-Z", "Numb (Live)");
        assert_eq!(ways, vec![("Linkin Park & Jay-Z".into(), "Numb (Live)".into()), ("Linkin Park & Jay-Z".into(), "Numb".into()), ("Linkin Park".into(), "Numb".into())]);
        assert_eq!(without_credits("[00:00.00] 作词 : X\n[00:01.00] Hi"), "[00:01.00] Hi");
    }
}

// ---------- Making timing for lyrics that have none ----------

// A first guess: the lines spread over the song, longer lines getting more time,
// leaving the intro and the ending free
pub fn estimate(lines: &[String], duration: f32) -> Vec<f32> {
    let duration = if duration > 0.0 { duration } else { 180.0 };
    let (start, end) = ((duration * 0.08).min(20.0), duration * 0.92);
    let weights: Vec<f32> = lines.iter().map(|line| 1.5 + line.chars().count() as f32 / 10.0).collect();
    let total: f32 = weights.iter().sum::<f32>().max(1.0);
    let mut at = start;
    weights.iter().map(|w| { let time = at; at += w / total * (end - start); time }).collect()
}

// A tap set line k to time t: the lines after it that weren't tapped follow, at the pace
// the taps so far show (sung faster or slower than the guess)
pub fn retime(times: &mut [f32], guesses: &[f32], tapped: &[bool], k: usize, t: f32) {
    times[k] = t;
    let earlier = (0..k).rev().find(|&i| tapped[i] && guesses[k] > guesses[i]);
    let pace = earlier.map_or(1.0, |i| ((t - times[i]) / (guesses[k] - guesses[i])).clamp(0.4, 2.5));
    for j in k + 1..times.len() {
        if !tapped[j] {
            times[j] = t + (guesses[j] - guesses[k]) * pace;
        }
    }
}

// Timed lines back to .lrc text
pub fn to_lrc(lines: &[Line]) -> String {
    lines.iter().map(|line| format!("[{}] {}", lrc_time(line.time as f64 * 1000.0), line.text)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod making_timing {
    #[test]
    fn taps_set_the_pace() {
        let lines: Vec<String> = ["one", "two", "three", "four"].iter().map(|s| s.to_string()).collect();
        let guesses = super::estimate(&lines, 100.0);
        assert!(guesses[0] >= 8.0 && guesses.windows(2).all(|w| w[1] > w[0]));
        let mut times = guesses.clone();
        let mut tapped = vec![false; 4];
        tapped[0] = true;
        super::retime(&mut times, &guesses, &tapped, 0, 20.0); // the singing starts later than guessed
        assert_eq!(times[0], 20.0);
        assert!((times[1] - (20.0 + guesses[1] - guesses[0])).abs() < 0.01);
        tapped[1] = true;
        let twice_as_slow = 20.0 + (guesses[1] - guesses[0]) * 2.0;
        super::retime(&mut times, &guesses, &tapped, 1, twice_as_slow);
        assert!((times[2] - (twice_as_slow + (guesses[2] - guesses[1]) * 2.0)).abs() < 0.01);
    }
}

#[cfg(test)]
mod local_first {
    #[test]
    fn lyrics_on_this_computer_win() {
        let dir = std::env::temp_dir().join("local-lyrics");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lyrics")).unwrap();
        std::fs::write(dir.join("Song.lrc"), "[00:01.00] Next to the song").unwrap();
        std::fs::write(dir.join("lyrics/Band - Song.txt"), "Saved").unwrap();
        let query = super::Query {
            path: dir.join("Song.mp3"), lyrics_dir: dir.join("lyrics"), file_name: "Band - Song".into(),
            artist: "Band".into(), title: "Song".into(), album: String::new(), duration: 100.0, cache_dir: dir.join("lyrics-cache"),
        };
        // An online source picked for the song doesn't win over them
        assert_eq!(super::find(&query, Some("lrclib"), false).source, "lrc");
        assert_eq!(super::find(&query, None, false).source, "lrc");
        assert_eq!(super::find(&query, Some("saved"), false).source, "saved");
        std::fs::remove_file(dir.join("Song.lrc")).unwrap();
        assert_eq!(super::find(&query, Some("netease"), false).source, "saved");
    }
}

#[cfg(test)]
mod remembering {
    use super::*;

    fn query(dir: &Path) -> Query {
        Query {
            path: dir.join("Song.mp3"), lyrics_dir: dir.join("lyrics"), cache_dir: dir.join("lyrics-cache"), file_name: "Band - Song".into(),
            artist: "Band".into(), title: "Song".into(), album: String::new(), duration: 100.0,
        }
    }

    #[test]
    fn answers_are_asked_once() {
        let dir = std::env::temp_dir().join(format!("lyrics-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let query = query(&dir);
        let timed = Found { synced: Some("[00:01.00] One\n[00:02.00] Two".into()), source: "lrclib".into(), source_name: "LRCLIB".into(), ..Default::default() };
        let asked = std::cell::Cell::new(0);
        let ask = |found: Option<Found>, reached_all: bool| {
            asked.set(asked.get() + 1);
            Online { found, reached_all, reached_any: reached_all }
        };
        // Found: kept, with its source, and not asked for again
        assert!(remembered(&query, "best", false, || ask(Some(timed.clone()), true)).found.is_some());
        let again = remembered(&query, "best", false, || ask(None, true)).found.unwrap();
        assert_eq!((asked.get(), again.synced, again.source.as_str(), again.saved), (1, timed.synced.clone(), "lrclib", false));
        // ...unless asked to look again
        assert!(remembered(&query, "best", true, || ask(None, true)).found.is_none());
        assert_eq!(asked.get(), 2);
        // "Nothing found" is kept too, but not when a source couldn't be reached
        assert!(remembered(&query, "best", false, || ask(Some(timed.clone()), true)).found.is_none());
        assert_eq!(asked.get(), 2);
        let other = Query { title: "Other".into(), ..query.clone() };
        assert!(remembered(&other, "best", false, || ask(None, false)).found.is_none());
        assert!(remembered(&other, "best", false, || ask(Some(timed.clone()), true)).found.is_some());
        assert_eq!(asked.get(), 4);
        // Another length is another version of the song, and a picked source has its own answer
        assert!(read_cache(&cache_file(&Query { duration: 250.0, ..other.clone() }, "best")).is_none());
        assert!(read_cache(&cache_file(&other, "netease")).is_none());
        // Old "nothing found" and second-best finds are asked again; timed lyrics stay
        let old = |file: &Path| {
            let mut value: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
            value["checked"] = serde_json::json!(now() - ASK_AGAIN - 1);
            std::fs::write(file, value.to_string()).unwrap();
        };
        old(&cache_file(&query, "best"));
        assert!(read_cache(&cache_file(&query, "best")).is_none());
        old(&cache_file(&other, "best"));
        assert!(read_cache(&cache_file(&other, "best")).is_some());
        write_cache(&cache_file(&other, "best"), &other, Some(&Found { plain: Some("Words".into()), ..Default::default() }));
        assert!(read_cache(&cache_file(&other, "best")).is_some());
        old(&cache_file(&other, "best"));
        assert!(read_cache(&cache_file(&other, "best")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Lyrics inside a song file are found before anything online. Needs an mp3 (it's
    // copied, never changed):
    //   MUSIC_PLAYER_TEST_MP3=some.mp3 cargo test -- --ignored
    #[test]
    #[ignore]
    fn lyrics_inside_the_file() {
        use lofty::file::{AudioFile, TaggedFileExt};
        use lofty::tag::{ItemKey, Tag};
        let source = std::fs::read(std::env::var("MUSIC_PLAYER_TEST_MP3").expect("MUSIC_PLAYER_TEST_MP3")).unwrap();
        let dir = std::env::temp_dir().join(format!("lyrics-inside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let query = query(&dir);
        for (text, timed) in [("First line\nSecond line", false), ("[00:01.00] One\n[00:02.00] Two\n[00:03.00] Three", true)] {
            std::fs::write(&query.path, &source).unwrap();
            let mut file = lofty::read_from_path(&query.path).unwrap();
            file.clear();
            assert!(embedded(&query.path).is_none() || file.save_to_path(&query.path, lofty::config::WriteOptions::default()).is_ok());
            let mut tag = Tag::new(file.primary_tag_type());
            assert!(tag.insert_text(ItemKey::UnsyncLyrics, text.to_string()), "mp3 tags keep lyrics under this name");
            file.insert_tag(tag);
            file.save_to_path(&query.path, lofty::config::WriteOptions::default()).unwrap();
            let found = find(&query, None, false);
            assert_eq!((found.source.as_str(), found.synced.is_some(), found.plain.is_some()), ("tags", timed, !timed));
            assert_eq!(found.plain.or(found.synced).unwrap(), text);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serbian_names() {
        use crate::names::{same_name, to_latin, without_accents};
        assert_eq!(to_latin("Рибља Чорба"), "Riblja Čorba");
        assert_eq!(to_latin("Џез и Љубав"), "Džez i Ljubav");
        assert_eq!(to_latin("Riblja Čorba"), "Riblja Čorba");
        assert_eq!(without_accents("Đorđe Balašević"), "Dorde Balasevic");
        assert_eq!(without_accents("Ноћни воз"), "Nocni voz");
        assert!(same_name("Рибља чорба", "Riblja Corba"));
        assert_eq!(variants("Riblja Čorba", "Noćni voz (Live)"), vec![
            ("Riblja Čorba".to_string(), "Noćni voz (Live)".to_string()),
            ("Riblja Čorba".to_string(), "Noćni voz".to_string()),
            ("Riblja Corba".to_string(), "Nocni voz".to_string()),
        ]);
    }

    // The whole way through a pretend Tekstovi.net (a little web server here): an artist
    // typed without accents is found under its accented letter, then the song, then its words
    #[test]
    fn a_song_on_tekstovi() {
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
                let path = line.split_whitespace().nth(1).unwrap_or("").replace("%2C", ",").replace("%C5%BD", "Ž");
                log.lock().unwrap().push(path.clone());
                let body = match path.as_str() {
                    "/2,Z,0.html" => r#"<html><a href="2,5,0.html">Zana</a></html>"#.to_string(),
                    "/2,Ž,0.html" => r#"<html><a href="2,77,0.html">Željko Samardžić</a> <a href="2,78,0.html">Željko Joksimović</a></html>"#.to_string(),
                    "/2,78,0.html" => r#"<html><a href="2,78,900.html">Druga</a><a href="2,78,901.html">Noćni voz <small>(12)</small></a></html>"#.to_string(),
                    "/2,78,901.html" => r#"<html><h1>Noćni voz</h1><p class="blurb">O pjesmi.</p><p class="lyric">Prvi red<br>Drugi red<br><br>Ref.<br>Treći red<br>Četvrti red</p><div>21.11.2026<br>Prijavi</div></html>"#.to_string(),
                    _ => String::new(),
                };
                let status = if body.is_empty() { "404 Not Found" } else { "200 OK" };
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        let base = format!("http://127.0.0.1:{port}");
        let dir = std::env::temp_dir();
        let song = Query { artist: "Zeljko Joksimovic".into(), title: "Nocni Voz (Live)".into(), ..query(&dir) };
        let found = tekstovi_at(&base, &song).unwrap().unwrap();
        assert_eq!(found.plain.unwrap(), "Prvi red\nDrugi red\n\nRef.\nTreći red\nČetvrti red");
        assert_eq!((found.source.as_str(), found.synced), ("tekstovi", None));
        assert_eq!(*asked.lock().unwrap(), vec!["/2,Z,0.html", "/2,Ž,0.html", "/2,78,0.html", "/2,78,901.html"]);
        // A song the artist doesn't have there, and an artist who isn't there: nothing, no error
        assert!(tekstovi_at(&base, &Query { title: "Nema je".into(), ..song.clone() }).unwrap().is_none());
        assert!(tekstovi_at(&base, &Query { artist: "Zzz".into(), ..song.clone() }).unwrap().is_none());
        // (the lists of artists weren't asked for again)
        assert_eq!(asked.lock().unwrap().iter().filter(|path| path.ends_with(",0.html") && !path.contains("78")).count(), 2);
    }

    #[test]
    fn a_page_of_tekstovi() {
        // (made up, in the shape the site's pages have)
        let index = r#"<ul><li><a href="2,293,0.html">Riblja Čorba</a></li><li><a class="x" href='https://tekstovi.net/2,294,0.html'>Regina</a></li></ul><a href="3,0,0.html">Dodaj tekst</a>"#;
        assert_eq!(tekstovi_links(index), vec![("293".to_string(), "0".to_string(), "Riblja Čorba".to_string()), ("294".to_string(), "0".to_string(), "Regina".to_string())]);
        let songs = r#"<a href="2,293,5512.html">Pogledaj dom svoj, an&#273;ele <span>(66)</span></a>"#;
        assert_eq!(tekstovi_links(songs)[0].1, "5512");
        assert_eq!(tekstovi_links(songs)[0].2, "Pogledaj dom svoj, anđele");
        let marked = r#"<div><p class="summary">About the song.</p><p class="lyric">First line<br>Second line<br /><br>Ref.<br>Third &amp; fourth<br>Fifth</p><p class="lyricCapt">x</p></div>"#;
        assert_eq!(tekstovi_lyrics(marked).unwrap(), "First line\nSecond line\n\nRef.\nThird & fourth\nFifth");
        let unmarked = "<div><p>About the song.</p><div>One<br>Two<br>Three<br><br>Four<br>Five<br>Six<br>Seven</div><div>21.11.2026<br>x</div></div>";
        assert_eq!(tekstovi_lyrics(unmarked).unwrap(), "One\nTwo\nThree\n\nFour\nFive\nSix\nSeven");
        assert!(tekstovi_lyrics("<div><p>Nothing here<br>really</p></div>").is_none());
    }
}
