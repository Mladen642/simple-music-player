// Everything the player keeps on disk, in ~/.local/share/music-player:
//
//   library.json      one record per song: names you gave it, its heart, its plays...
//                     (the same file the web version used, so your library carries over)
//   settings.json     the default music folder
//   preferences.json  theme, volume, sorting, shuffle
//   covers/, lyrics/  covers found online, lyrics you saved
//
// Records are kept by "file name|size", so a song keeps its record wherever it moves.
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// Where everything is kept. MUSIC_PLAYER_DATA=/some/folder picks another place.
pub fn data_dir() -> PathBuf {
    let folder = match std::env::var_os("MUSIC_PLAYER_DATA") {
        Some(chosen) => PathBuf::from(chosen),
        None => dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("music-player"),
    };
    let _ = fs::create_dir_all(&folder);
    folder
}

// Writes a temporary file first and then swaps it in, so a crash or a power cut can
// never leave half a file behind
pub fn write_safely(file: &Path, text: &str) -> std::io::Result<()> {
    let temp = file.with_extension("json.tmp");
    fs::write(&temp, text)?;
    fs::rename(&temp, file)
}

fn read_json(file: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(file).ok()?).ok()
}

// One song's record. Fields this version doesn't know (from a newer one) are kept as they are.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Record {
    pub file: Option<String>,        // where the song is inside the music folder
    pub title: Option<String>,
    pub artist: Option<String>,      // "" is an unknown artist
    pub album: Option<String>,
    pub genres: Option<Vec<String>>,
    pub favorite: Option<String>,    // when it got its heart (so the newest favorites come first)
    pub plays: u64,
    pub lyrics: Option<String>,      // the lyrics source picked for this song
    pub added: Option<u64>,          // when the file came into the folder (milliseconds)
    pub edited: Vec<String>,         // the names you changed yourself
    pub other: Map<String, Value>,
}

impl Record {
    // Read leniently: a field with an unexpected value is skipped, never the whole library
    fn from_value(value: &Value) -> Record {
        let mut record = Record::default();
        let Some(map) = value.as_object() else { return record };
        let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_string);
        record.file = text("file");
        record.title = text("title");
        record.artist = text("artist");
        record.album = text("album");
        record.genres = map.get("genres").and_then(Value::as_array).map(|list| {
            list.iter().filter_map(Value::as_str).map(str::to_string).collect()
        });
        record.favorite = text("favorite");
        record.plays = map.get("plays").and_then(Value::as_f64).map_or(0, |n| n.max(0.0) as u64);
        record.lyrics = text("lyrics");
        record.added = map.get("added").and_then(Value::as_f64).map(|n| n.max(0.0) as u64);
        record.edited = map.get("edited").and_then(Value::as_array).map_or(Vec::new(), |list| {
            list.iter().filter_map(Value::as_str).map(str::to_string).collect()
        });
        const KNOWN: [&str; 10] = ["file", "title", "artist", "album", "genres", "favorite", "plays", "lyrics", "added", "edited"];
        for (key, value) in map {
            if !KNOWN.contains(&key.as_str()) {
                record.other.insert(key.clone(), value.clone());
            }
        }
        record
    }

    // Written in the same order as before, so library.json stays easy to read
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                map.insert(key.to_string(), value);
            }
        };
        put("file", self.file.clone().map(Value::from));
        put("title", self.title.clone().map(Value::from));
        put("artist", self.artist.clone().map(Value::from));
        put("album", self.album.clone().map(Value::from));
        put("genres", self.genres.clone().map(Value::from));
        put("favorite", self.favorite.clone().map(Value::from));
        put("plays", (self.plays > 0).then(|| Value::from(self.plays)));
        put("lyrics", self.lyrics.clone().map(Value::from));
        put("added", self.added.map(Value::from));
        put("edited", (!self.edited.is_empty()).then(|| Value::from(self.edited.clone())));
        for (key, value) in &self.other {
            map.insert(key.clone(), value.clone());
        }
        Value::Object(map)
    }
}

pub struct Library {
    pub songs: IndexMap<String, Record>,
    file: PathBuf,
    dirty_since: Option<Instant>,
    pub broken: bool, // library.json couldn't be read: nothing is written, so it isn't replaced
}

impl Library {
    pub fn load(data: &Path) -> Library {
        let file = data.join("library.json");
        let mut library = Library { songs: IndexMap::new(), file: file.clone(), dirty_since: None, broken: false };
        if !file.exists() {
            return library;
        }
        match read_json(&file) {
            Some(value) => {
                if let Some(songs) = value.get("songs").and_then(Value::as_object) {
                    for (key, record) in songs {
                        library.songs.insert(key.clone(), Record::from_value(record));
                    }
                }
                // A copy as it was at the start, in case something goes wrong later
                let _ = fs::copy(&file, data.join("library.backup.json"));
            }
            None => {
                eprintln!("library.json can't be read; it won't be changed until it's fixed.");
                library.broken = true;
            }
        }
        library
    }

    pub fn get(&self, id: &str) -> Option<&Record> {
        self.songs.get(id)
    }

    // Changes a record (making it if it's new) and saves soon after
    pub fn change(&mut self, id: &str, change: impl FnOnce(&mut Record)) {
        let record = self.songs.entry(id.to_string()).or_default();
        let before = record.clone();
        change(record);
        if *record != before {
            self.dirty_since.get_or_insert_with(Instant::now);
        }
    }

    // Something changed in a way change() can't see (a record moved or went away)
    pub fn mark_changed(&mut self) {
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    // Saves when there are changes, at most every two seconds (or right away with force)
    pub fn save_if_needed(&mut self, force: bool) {
        let Some(since) = self.dirty_since else { return };
        if self.broken || (!force && since.elapsed() < Duration::from_secs(2)) {
            return;
        }
        let songs: Map<String, Value> = self.songs.iter().map(|(key, record)| (key.clone(), record.to_value())).collect();
        let text = serde_json::to_string_pretty(&json!({ "version": 1, "songs": songs })).unwrap_or_default();
        match write_safely(&self.file, &text) {
            Ok(()) => self.dirty_since = None,
            Err(error) => eprintln!("Couldn't save library.json: {error}"),
        }
    }
}

// ---------- settings.json (the default folder, shared with the web version) ----------

pub fn default_folder(data: &Path) -> Option<PathBuf> {
    let settings = read_json(&data.join("settings.json"))?;
    let folder = settings.get("defaultFolder")?.as_str()?;
    (!folder.is_empty()).then(|| PathBuf::from(folder))
}

pub fn set_default_folder(data: &Path, folder: &Path) {
    let file = data.join("settings.json");
    let mut settings = read_json(&file).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    settings.insert("defaultFolder".into(), Value::from(folder.to_string_lossy().to_string()));
    let _ = write_safely(&file, &serde_json::to_string_pretty(&Value::Object(settings)).unwrap_or_default());
}

// Your Music folder (its name depends on the system's language)
pub fn music_folder() -> PathBuf {
    dirs::audio_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Music"))
}

// ---------- preferences.json (how the player looks and plays) ----------

#[derive(Clone, PartialEq)]
pub struct Preferences {
    pub theme: String,
    pub volume: f32,
    pub muted: bool,
    pub sort: String,
    pub shuffle: bool,
    pub stats_genres: bool,     // the statistics show genres instead of artists
    pub only_favorites: bool,
    pub even_volume: bool,      // every song at about the same loudness
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            theme: "light".into(),
            volume: 0.8,
            muted: false,
            sort: "folder".into(),
            shuffle: false,
            stats_genres: false,
            only_favorites: false,
            even_volume: true,
        }
    }
}

impl Preferences {
    // None on the very first start (the welcome window then offers to bring the old library)
    pub fn load(data: &Path) -> Option<Preferences> {
        let value = read_json(&data.join("preferences.json"))?;
        let d = Preferences::default();
        let text = |key: &str, default: &str| value.get(key).and_then(Value::as_str).unwrap_or(default).to_string();
        let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
        Some(Preferences {
            theme: text("theme", &d.theme),
            volume: value.get("volume").and_then(Value::as_f64).map_or(d.volume, |v| v.clamp(0.0, 1.0) as f32),
            muted: flag("muted"),
            sort: text("sort", &d.sort),
            shuffle: flag("shuffle"),
            stats_genres: flag("statsGenres"),
            only_favorites: flag("onlyFavorites"),
            even_volume: value.get("evenVolume").and_then(Value::as_bool).unwrap_or(true),
        })
    }

    pub fn save(&self, data: &Path) {
        let value = json!({
            "theme": self.theme, "volume": self.volume, "muted": self.muted, "sort": self.sort,
            "shuffle": self.shuffle, "statsGenres": self.stats_genres, "onlyFavorites": self.only_favorites, "evenVolume": self.even_volume,
        });
        let _ = write_safely(&data.join("preferences.json"), &serde_json::to_string_pretty(&value).unwrap_or_default());
    }
}

// ---------- Bringing the library from the old (web) player ----------

// The old player kept everything next to its server.py. Copies what's there; says what
// it found, or why it couldn't.
pub fn import_old_player(old: &Path, data: &Path) -> Result<String, String> {
    if !old.join("library.json").is_file() {
        return Err("That folder has no library.json. Pick the old player's folder (with server.py in it), or a Music Player data folder.".into());
    }
    if read_json(&old.join("library.json")).is_none() {
        return Err("The old library.json can't be read.".into());
    }
    let files = ["library.json", "settings.json", "wikipedia.json", "loudness.json", "genres.json", "last-rename.json"];
    for name in files {
        if old.join(name).is_file() {
            fs::copy(old.join(name), data.join(name)).map_err(|e| format!("Couldn't copy {name}: {e}"))?;
        }
    }
    let mut copied = 0;
    for folder in ["covers", "lyrics"] {
        copied += copy_folder(&old.join(folder), &data.join(folder));
    }
    let songs = read_json(&data.join("library.json"))
        .and_then(|v| v.get("songs").and_then(Value::as_object).map(Map::len))
        .unwrap_or(0);
    Ok(format!("Your library is here: {songs} songs, with {copied} covers and saved lyrics."))
}

fn copy_folder(from: &Path, to: &Path) -> usize {
    let Ok(entries) = fs::read_dir(from) else { return 0 };
    let _ = fs::create_dir_all(to);
    entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter(|entry| fs::copy(entry.path(), to.join(entry.file_name())).is_ok())
        .count()
}

// The time right now, the way favorites are dated: "2026-10-11T16:48:00Z"
pub fn now_iso() -> String {
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (seconds / 86400, seconds % 86400);
    // Days since 1970 to a calendar date (Howard Hinnant's algorithm)
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(test)]
pub mod tests {
    use std::path::{Path, PathBuf};

    // A small library like the old player kept, in a temporary folder
    pub fn sample_library(name: &str) -> PathBuf {
        let old = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&old);
        std::fs::create_dir_all(old.join("covers")).unwrap();
        std::fs::create_dir_all(old.join("lyrics")).unwrap();
        std::fs::write(old.join("library.json"), r#"{"version": 1, "songs": {
            "Band - Song.mp3|1000": {"file": "Band - Song.mp3", "title": "Song", "artist": "Band", "album": "", "genres": ["Rock"],
                                     "favorite": "2026-01-01T10:00:00Z", "plays": 7, "edited": ["genres"], "future": {"kept": true}},
            "Other.mp3|2000": {"file": "Other.mp3", "title": "Other", "artist": "", "plays": 1}
        }}"#).unwrap();
        std::fs::write(old.join("settings.json"), r#"{"defaultFolder": "/nowhere"}"#).unwrap();
        std::fs::write(old.join("covers/abc.jpg"), b"picture").unwrap();
        std::fs::write(old.join("lyrics/Band - Song.lrc"), "[00:01.00] Words").unwrap();
        old
    }

    #[test]
    fn import_from_the_old_player() {
        let old = sample_library("old-player-test");
        let data: &Path = &std::env::temp_dir().join("imported-test");
        let _ = std::fs::remove_dir_all(data);
        std::fs::create_dir_all(data).unwrap();
        let message = super::import_old_player(&old, data).unwrap();
        assert!(message.contains("2 songs"), "{message}");
        let mut library = super::Library::load(data);
        let record = library.get("Band - Song.mp3|1000").unwrap().clone();
        assert_eq!((record.plays, record.favorite.is_some(), record.edited.clone()), (7, true, vec!["genres".to_string()]));
        assert!(data.join("covers/abc.jpg").exists() && data.join("lyrics/Band - Song.lrc").exists());
        // Written back without losing anything (a field from a newer version included)
        library.change("Other.mp3|2000", |r| r.plays += 1);
        library.change("Other.mp3|2000", |r| r.plays -= 1);
        library.save_if_needed(true);
        let before: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(old.join("library.json")).unwrap()).unwrap();
        let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(data.join("library.json")).unwrap()).unwrap();
        assert_eq!(before["songs"], after["songs"]);
    }
}
