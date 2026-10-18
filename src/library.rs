// The songs in the music folder: finding them, reading their tags in the background,
// and noticing when songs are added or removed (the folder is checked every few seconds).
use crate::names::{self, Names, UNKNOWN_ARTIST};
use crate::store::{Library, Record};
use lofty::config::ParseOptions;
use lofty::prelude::*;
use lofty::probe::Probe;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, UNIX_EPOCH};

pub const AUDIO_EXTENSIONS: [&str; 9] = ["mp3", "m4a", "aac", "flac", "ogg", "oga", "opus", "wav", "webm"];
const IMAGE_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "gif"];
const FOLDER_CHECK: Duration = Duration::from_secs(5);

// A file in the music folder
#[derive(Clone, PartialEq, Debug)]
pub struct Entry {
    pub rel: String, // its place inside the folder, with "/" ("Album/Song.mp3")
    pub size: u64,
    pub modified: u64, // milliseconds since 1970
}

// What the folder holds: songs, and one picture per folder for songs without their own
#[derive(Clone, PartialEq, Default)]
pub struct Listing {
    pub songs: Vec<Entry>,
    pub folder_covers: HashMap<String, String>, // folder ("" for the top) -> image file
}

pub struct Song {
    pub id: String, // "file name|size": its key in library.json
    pub rel: String,
    pub path: PathBuf,
    pub size: u64,
    pub folder: String,
    pub modified: u64,
    pub tags: Names,       // as stored in the file
    pub tags_read: bool,
    pub duration: f32,     // seconds (0 until the tags are read)
    pub bitrate: u32,      // kbps (for picking the best copy of a song)
    pub natural: Names,    // the file's own names, tidied
    // What the player shows: library.json wins over the file's own names
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub added: u64,
    pub search: String,    // title, artist and album without accents, for searching
}

impl Song {
    pub fn new(entry: &Entry, root: &Path) -> Song {
        let file_name = entry.rel.rsplit('/').next().unwrap_or(&entry.rel);
        let mut song = Song {
            id: format!("{file_name}|{}", entry.size),
            rel: entry.rel.clone(),
            path: root.join(&entry.rel),
            size: entry.size,
            folder: entry.rel.rsplit_once('/').map_or(String::new(), |(folder, _)| folder.to_string()),
            modified: entry.modified,
            tags: Names::default(),
            tags_read: false,
            duration: 0.0,
            bitrate: 0,
            natural: Names::default(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            genre: String::new(),
            added: entry.modified,
            search: String::new(),
        };
        song.natural = names::natural_names(&song.tags, &song.rel);
        song
    }

    // The names to show: what library.json says wins over tags and the file name
    pub fn apply_record(&mut self, record: Option<&Record>) {
        let natural = self.natural.clone();
        self.title = natural.title;
        self.artist = natural.artist;
        self.album = natural.album;
        self.genre = natural.genre;
        self.added = self.modified;
        if let Some(record) = record {
            if let Some(title) = record.title.as_ref().filter(|t| !t.is_empty()) {
                self.title = title.clone();
            }
            if let Some(artist) = &record.artist {
                self.artist = if artist.is_empty() { UNKNOWN_ARTIST.into() } else { artist.clone() };
            }
            if let Some(album) = &record.album {
                self.album = album.clone();
            }
            if let Some(genres) = &record.genres {
                self.genre = genres.join(", ");
            }
            if let Some(added) = record.added {
                self.added = added; // writing tags gave the file a new date
            }
        }
        self.search = names::normalize(&format!("{} {} {}", self.title, self.artist, self.album));
    }

    pub fn known_artist(&self) -> bool {
        self.artist != UNKNOWN_ARTIST
    }
}

// The first time a song's tags are read, what the player shows for it becomes its record.
// Only what library.json doesn't have yet is added, so nothing saved is ever replaced.
pub fn remember_song(library: &mut Library, song: &Song) {
    let natural = &song.natural;
    library.change(&song.id, |record| {
        // A title or artist that came from the file (not one you changed yourself) follows
        // the naming rules as they are now, so better rules reach songs named earlier too
        if record.title.is_some() && !record.edited.iter().any(|f| f == "title") && song.tags_read {
            record.title = Some(natural.title.clone());
        }
        if record.artist.is_some() && !record.edited.iter().any(|f| f == "artist") && song.tags_read {
            record.artist = Some(if natural.artist == UNKNOWN_ARTIST { String::new() } else { natural.artist.clone() });
        }
        if record.file.as_deref() != Some(song.rel.as_str()) {
            record.file = Some(song.rel.clone());
        }
        record.title.get_or_insert_with(|| natural.title.clone());
        record.artist.get_or_insert_with(|| if natural.artist == UNKNOWN_ARTIST { String::new() } else { natural.artist.clone() });
        record.album.get_or_insert_with(|| natural.album.clone());
        record.genres.get_or_insert_with(|| names::genres_of(&natural.genre).iter().map(|g| g.to_string()).collect());
    });
}

// ---------- The folder ----------

// Every song file and picture in the folder and its subfolders (hidden folders left out)
pub fn scan(root: &Path) -> Listing {
    let mut listing = Listing::default();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                folders.push(path);
                continue;
            }
            let extension = name.rsplit_once('.').map_or(String::new(), |(_, ext)| ext.to_lowercase());
            let Ok(rel) = path.strip_prefix(root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if AUDIO_EXTENSIONS.contains(&extension.as_str()) {
                let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
                listing.songs.push(Entry { rel, size: meta.len(), modified });
            } else if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
                // One picture per folder, preferring names like cover.jpg or folder.jpg
                let folder = rel.rsplit_once('/').map_or(String::new(), |(f, _)| f.to_string());
                let looks_like_cover = ["cover", "folder", "front", "album"].iter().any(|word| name.to_lowercase().contains(word));
                if looks_like_cover || !listing.folder_covers.contains_key(&folder) {
                    listing.folder_covers.insert(folder, rel);
                }
            }
        }
    }
    // "Folder order": the way a file manager lists them
    listing.songs.sort_by_cached_key(|entry| entry.rel.to_lowercase());
    listing
}

// Checks the folder every few seconds and sends its listing whenever it changed.
// Stops by itself when another folder is opened (generation goes up).
pub fn watch(root: PathBuf, generation: Arc<AtomicU64>, mine: u64, paused: Arc<std::sync::atomic::AtomicBool>, send: Sender<crate::app::Event>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let mut last = None;
        while generation.load(Ordering::Relaxed) == mine {
            if paused.load(Ordering::Relaxed) {
                // the player is renaming or tagging files: it updates the songs itself
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
            let listing = scan(&root);
            if last.as_ref() != Some(&listing) {
                last = Some(listing.clone());
                if send.send(crate::app::Event::Listing(mine, listing)).is_err() {
                    return;
                }
                wake();
            }
            std::thread::sleep(FOLDER_CHECK);
        }
    });
}

// ---------- Tags ----------

pub struct TagResult {
    pub id: String,
    pub tags: Names,
    pub duration: f32,
    pub bitrate: u32,
}

// Reads title, artist, album, genre and length (not the cover: that's read when needed)
pub fn read_tags(path: &Path) -> (Names, f32, u32) {
    let options = ParseOptions::new().read_cover_art(false);
    let file = Probe::open(path).ok().and_then(|probe| probe.options(options).guess_file_type().ok()).and_then(|probe| probe.read().ok());
    let Some(file) = file else {
        return (Names::default(), 0.0, 0);
    };
    let bitrate = file.properties().audio_bitrate().unwrap_or(0);
    let duration = file.properties().duration().as_secs_f32();
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return (Names::default(), duration, bitrate);
    };
    let get = |value: Option<std::borrow::Cow<str>>| value.map(|v| v.trim().to_string()).unwrap_or_default();
    let names = Names { title: get(tag.title()), artist: get(tag.artist()), album: get(tag.album()), genre: get(tag.genre()) };
    (names, duration, bitrate)
}

// Reads the tags of the songs one by one in the background
pub fn read_tags_in_background(songs: Vec<(String, PathBuf)>, send: Sender<crate::app::Event>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        for (index, (id, path)) in songs.iter().enumerate() {
            let (tags, duration, bitrate) = read_tags(path);
            if send.send(crate::app::Event::Tags(TagResult { id: id.clone(), tags, duration, bitrate })).is_err() {
                return;
            }
            if index % 25 == 0 {
                wake();
            }
        }
        wake();
    });
}
