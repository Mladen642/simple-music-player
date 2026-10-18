// Lyrics. Where they come from, in this order ("Best match"):
//   1. a .lrc file next to the song,
//   2. the lyrics folder (lyrics you saved),
//   3. LRCLIB, a free lyrics library,
//   4. lrcmux, a free service that asks YouTube Music, Genius, KuGou and LRCLIB at once,
//   5. NetEase Cloud Music, which has timed lyrics for a great many songs.
// Timed lyrics win over plain text, even those of a slightly different version of the
// song (the timing buttons line them up). Names are tried several ways, since lyrics
// libraries often spell them differently than the tags do. The menu above the lyrics picks one source instead;
// that choice is kept per song in library.json.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const SOURCES: [(&str, &str); 7] = [
    ("lrc", ".lrc file"),
    ("saved", "Lyrics folder"),
    ("lrclib", "LRCLIB"),
    ("ytmusic", "YouTube Music"), // these three are asked through lrcmux
    ("genius", "Genius"),
    ("kugou", "KuGou"),
    ("netease", "NetEase"),
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
    pub file_name: String, // "Artist - Title", for the lyrics folder
    pub artist: String,    // empty when unknown
    pub title: String,
    pub album: String,     // empty unless really known
    pub duration: f32,
}

// The song's own lyrics first, then online (choice: a source picked for this song)
pub fn find(query: &Query, choice: Option<&str>) -> Found {
    // Lyrics on this computer always win: a .lrc next to the song or saved ones (the one
    // picked for this song first, when it's one of these two), never replaced by online ones
    let local = match choice {
        Some("saved") => ["saved", "lrc"],
        _ => ["lrc", "saved"],
    };
    for id in local {
        if let Ok(Some(found)) = from_source(query, id) {
            return found;
        }
    }
    if let Some(choice) = choice.filter(|c| !local.contains(c)) {
        if let Ok(Some(found)) = from_source(query, choice) {
            if found.has_lyrics() {
                return found;
            }
        }
    }
    let mut fallback: Option<Found> = None;
    let mut reached_any = false;
    for id in ["lrclib", "best", "netease"] {
        // "best": lrcmux picks from all its sources
        match from_source(query, id) {
            Ok(found) => {
                reached_any = true;
                if let Some(found) = found {
                    if (found.synced.is_some() && !found.other_version) || found.message.is_some() {
                        return found; // timed lyrics, or "Instrumental"
                    }
                    if fallback.as_ref().is_none_or(|f| rank(&found) > rank(f)) {
                        fallback = Some(found);
                    }
                }
            }
            Err(error) => eprintln!("Lyrics from {id}: {error}"),
        }
    }
    fallback.unwrap_or_else(|| Found {
        message: Some(if reached_any { "No lyrics found for this song.".into() } else { "Couldn't reach the lyrics services. Check your internet connection.".into() }),
        ..Default::default()
    })
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
// ("A & B", "A feat. B" -> "A") and the title without "(Live)" or "feat. X"
pub fn variants(artist: &str, title: &str) -> Vec<(String, String)> {
    use std::sync::LazyLock;
    static MORE_ARTISTS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)\s*(?:&|,|\+|\bx\b|\band\b|\bvs\.?|\bfeat\.?|\bft\.?|\bfeaturing\b|\bwith\b)\s.*$").unwrap());
    let first_artist = MORE_ARTISTS.replace(artist, "").trim().to_string();
    let simple_title = crate::names::lookup_name(title);
    let mut list = vec![(artist.to_string(), title.to_string()), (artist.to_string(), simple_title.clone())];
    if !first_artist.is_empty() && first_artist != artist {
        list.push((first_artist.clone(), simple_title.clone()));
    }
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
        "lrclib" => lrclib(query),
        "netease" => netease(query),
        _ => lrcmux(query, if id == "best" { "" } else { id }),
    }
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

fn agent(seconds: u64) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(seconds)).user_agent("SimpleMusicPlayer/2.0").build()
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
        if let Some(item) = ask("get", &params)? {
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
    let credit = regex::Regex::new(r"^\s*(\[[^\]]*\])+\s*(作词|作曲|编曲|制作人|混音|母带|和声|吉他|贝斯|鼓|出品|发行|监制|录音)\s*[:：]").unwrap();
    lrc.lines().filter(|line| !credit.is_match(line)).collect::<Vec<_>>().join("\n")
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
        Err(ureq::Error::Status(_, _)) => return Ok(None), // not found (404) or too many requests (429)
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
    let info = regex::Regex::new(r"(?i)^\[[a-z]+:").unwrap();
    lines.extend(synced.lines().filter(|row| !info.is_match(row)).map(str::to_string));
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
            artist: "Band".into(), title: "Song".into(), album: String::new(), duration: 100.0,
        };
        // An online source picked for the song doesn't win over them
        assert_eq!(super::find(&query, Some("lrclib")).source, "lrc");
        assert_eq!(super::find(&query, None).source, "lrc");
        assert_eq!(super::find(&query, Some("saved")).source, "saved");
        std::fs::remove_file(dir.join("Song.lrc")).unwrap();
        assert_eq!(super::find(&query, Some("netease")).source, "saved");
    }
}
