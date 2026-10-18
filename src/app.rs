// The app: what it knows and what it does. Drawing is in ui.rs.
//
// Background workers (folder watching, tag reading, covers, media keys) never touch the
// app directly: they send Events, which the app picks up at the start of every frame.
use crate::analysis::Spectrum;
use crate::covers::{self, Covers};
use crate::loudness::{Measured, Meter};
use crate::lyrics::{self, Found};
use crate::library::{self, Listing, Song, TagResult};
use crate::media::{Command, Media};
use crate::names::{self, UNKNOWN_ARTIST};
use crate::player::Player;
use crate::store::{self, Library, Preferences};
use crate::theme::{self, Theme};
use eframe::egui;
use std::cmp::Ordering as Order;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

// Startup checkpoints, printed when MP_DEBUG is set (for finding problems)
pub fn debug(what: &str) {
    if std::env::var_os("MP_DEBUG").is_some() {
        eprintln!("[{:?}] {what}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() % 100000);
    }
}

pub enum Event {
    Listing(u64, Listing),
    Tags(TagResult),
    Cover(covers::Cover),
    Media(Command),
    Lyrics(String, Found),       // the song's id, and what was found
    Tidied(Vec<Done>),           // files "Tidy file names" changed in the background
    Wiki(String, bool),          // Wikipedia answered for this song key (false: couldn't be reached)
    ToolsReady(Result<(), String>),                                  // yt-dlp, ffmpeg and deno downloaded
    Checked(Result<(String, Vec<crate::downloads::Entry>), String>), // what a link holds
    Download(usize, crate::downloads::State),                        // a song's download moved on
    Loudness(String, Measured),
}

pub const SORTS: [(&str, &str); 7] = [
    ("folder", "Folder order"),
    ("title", "Title, A to Z"),
    ("artist", "By artist"),
    ("album", "By album"),
    ("genre", "By genre"),
    ("recent", "Recently added"),
    ("plays", "Most played"),
];

// A cover as pictures on the graphics card
#[derive(Default)]
pub struct CoverTextures {
    pub big: Option<egui::TextureHandle>,
    pub backdrop: Option<egui::TextureHandle>,
    pub small: Option<egui::TextureHandle>,
    pub none: bool, // no cover anywhere
}

// One line in "All songs": a song, or the name of a group (in sorts that have groups)
pub enum Row {
    Group { key: String, name: String, sub: String, count: usize },
    Song(usize),
}

pub struct Bar {
    pub key: String,
    pub name: String,
    pub songs: usize,
}

#[derive(Default)]
pub struct Stats {
    pub songs: usize,
    pub artists: Vec<Bar>, // most songs first, unknown last
    pub genres: Vec<Bar>,
    pub albums: usize,
}

// The lyrics on screen
pub struct LyricsView {
    pub id: String,                // the song they belong to
    pub found: Option<Found>,      // None while still looking
    pub lines: Vec<lyrics::Line>,  // timed lines (empty for plain lyrics)
    pub offset: f32,               // seconds the timed lines are moved (+ later), until saved
    pub scrolled_at: Option<Instant>, // when you scrolled them yourself (they stop following for a moment)
    pub tapping: Option<Tapping>,  // making the timing by tapping along
}

// Tapping along: each tap times the next line, and the lines after it follow the pace
pub struct Tapping {
    pub next: usize,      // the line the next tap is for
    pub guesses: Vec<f32>,
    pub tapped: Vec<bool>,
    history: Vec<(usize, Vec<f32>, Vec<bool>)>, // for "Undo"
}

// The pencil's window: a song's names as you type them
pub struct EditForm {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub rename: bool, // also give the file a matching name
}

// One suggestion of "Tidy file names"
pub struct Tidy {
    pub id: String,
    pub old_name: String,
    pub new_name: String, // the same as old_name when only the tags get the names
    pub tags: bool,       // also write the names you gave the song into its tags
    pub clash: bool,      // another file has that name already
    pub chosen: bool,
}

// What happened to one file while tidying
pub struct Done {
    pub id: String,
    pub old_rel: String,
    pub new_rel: String,            // the same as old_rel when it wasn't renamed
    pub modified: u64,
    pub tags: Option<names::Names>, // the names written into it
    pub failed: bool,
}

// The download window
#[derive(Default)]
pub struct Downloads {
    pub open: bool,
    pub tools: crate::downloads::Tools,
    pub getting_tools: bool,
    pub link: String,
    pub checking: bool,
    pub playlist: String,
    pub entries: Vec<crate::downloads::Entry>, // what the link holds (names can be changed before downloading)
    pub jobs: Vec<(crate::downloads::Entry, crate::downloads::State)>,
    pub error: Option<String>,
    running: crate::downloads::Running,
    queue: Arc<std::sync::Mutex<std::collections::VecDeque<(usize, crate::downloads::Entry)>>>,
    workers: Arc<std::sync::atomic::AtomicUsize>,
    updated: bool,
}

impl Downloads {
    pub fn active(&self) -> usize {
        use crate::downloads::State::*;
        self.jobs.iter().filter(|(_, state)| matches!(state, Waiting | Downloading(_) | Converting)).count()
    }
}

// What Wikipedia has for a song that the song doesn't (yet)
pub struct WikiSuggestion {
    pub id: String,
    pub album: Option<(String, bool)>,       // Wikipedia's album, and whether the song has none yet
    pub genres: Option<(Vec<String>, bool)>, // the same for genres
}

// The duplicates window: copies of one song, the best first
pub struct Duplicates {
    pub ids: Vec<String>,
    pub remove: Vec<bool>,
}

pub struct Welcome {
    pub message: Option<String>, // the result of importing, or why it didn't work
}

pub struct App {
    pub data: PathBuf,
    pub library: Library,
    pub prefs: Preferences,
    saved_prefs: Preferences,
    pub theme: &'static Theme,
    pub styled_theme: Option<&'static str>,

    // The music folder
    pub folder: PathBuf,
    pub folder_missing: Option<PathBuf>, // the default folder, when it isn't there right now
    generation: Arc<AtomicU64>,
    pub songs: Vec<Song>,
    by_id: HashMap<String, usize>,
    folder_covers: HashMap<String, String>,
    pub tags_waiting: usize,
    pub listed: bool, // the folder was read at least once

    // Playing
    pub player: Player,
    pub current: Option<String>, // the id of the song playing (or paused)
    queue: Vec<String>,          // what plays next
    ordered: Vec<String>,        // the list the queue came from, in its own order
    counted: Option<String>,     // the song whose play was counted already
    pub duration: f32,
    pub scroll_to_current: bool,

    // Covers
    covers: Covers,
    pub textures: HashMap<String, CoverTextures>,
    requested: HashSet<(String, bool)>,

    // What "All songs" shows
    pub artist_filter: Option<String>,
    pub genre_filter: Option<String>,
    pub rows: Vec<Row>,
    pub shown: Vec<usize>, // the songs in "All songs", in order
    view_dirty: bool,
    pub stats: Stats,
    stats_dirty: bool,

    // The rest of the screen
    pub search: String,
    pub search_results: Vec<usize>,
    searched: String,
    pub toast: Option<(String, Instant)>,
    pub welcome: Option<Welcome>,
    pub sort_open: bool,
    pub theme_open: bool,
    pub hover_theme: Option<&'static str>, // a theme being pointed at in the menu (a preview)
    pub focus_search: bool,
    pub lyrics: Option<LyricsView>,
    pub lyrics_menu_open: bool,
    pub editing: Option<EditForm>,
    pub tidy: Option<Vec<Tidy>>,
    pub deleting: Option<String>,
    pub duplicates: Option<Duplicates>,
    pub dup_groups: HashMap<String, Vec<String>>, // "artist|title" -> ids of copies (only groups of two or more)
    paused: Arc<std::sync::atomic::AtomicBool>,     // folder watching waits while files are being changed
    // Albums and genres from Wikipedia
    pub wiki: Arc<std::sync::Mutex<crate::wiki::Cache>>,
    wiki_line: Arc<(std::sync::Mutex<std::collections::VecDeque<(String, String, String)>>, std::sync::Condvar)>,
    wiki_started: bool,
    pub wiki_waiting: usize,
    pub wiki_offline: bool,
    pub wiki_open: bool,
    pub wiki_list: Vec<WikiSuggestion>,
    pub wiki_ticks: HashMap<(String, bool), bool>, // (song id, genres?) -> use it
    pub wiki_fill: Option<String>,                 // the pencil asked for this song's key
    pub downloads: Downloads,
    pub import_pending: Option<PathBuf>,           // a .zip to import, once you confirm
    wiki_saved_at: Instant,
    pub busy: bool,                                 // tidying in the background

    // Even volume, the equalizer and the background
    pub loudness: HashMap<String, Measured>,
    loudness_changed: Option<Instant>,
    meter: Meter,
    pub spectrum: Spectrum,
    pub eq_moving: bool,
    pub backdrop: Option<(String, egui::TextureHandle)>,         // the cover behind everything
    pub old_backdrop: Option<(egui::TextureHandle, Instant)>,     // the one fading out

    events: Receiver<Event>,
    pub send: Sender<Event>,
    media: Media,
    media_shown: (bool, Instant),
    pub ctx: egui::Context,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> App {
        debug("app: starting");
        crate::ui::setup_fonts(&cc.egui_ctx);
        debug("app: fonts ready");
        let ctx = cc.egui_ctx.clone();
        let data = store::data_dir();
        let (send, events) = channel();
        let first_start = Preferences::load(&data).is_none();
        let prefs = Preferences::load(&data).unwrap_or_default();
        debug("app: opening sound");
        let mut player = Player::new();
        debug("app: sound ready");
        player.set_volume(if prefs.muted { 0.0 } else { prefs.volume });

        let wake = |ctx: &egui::Context| {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let covers = Covers::start(data.join("covers"), send.clone(), wake(&ctx));
        debug("app: covers ready");
        let media = Media::start(send.clone(), wake(&ctx));
        debug("app: media keys ready");
        let meter = Meter::start(send.clone(), wake(&ctx));

        let mut app = App {
            library: Library::load(&data),
            theme: theme::by_id(&prefs.theme),
            styled_theme: None,
            saved_prefs: prefs.clone(),
            prefs,
            folder: PathBuf::new(),
            folder_missing: None,
            generation: Arc::new(AtomicU64::new(0)),
            songs: Vec::new(),
            by_id: HashMap::new(),
            folder_covers: HashMap::new(),
            tags_waiting: 0,
            listed: false,
            player,
            current: None,
            queue: Vec::new(),
            ordered: Vec::new(),
            counted: None,
            duration: 0.0,
            scroll_to_current: false,
            covers,
            textures: HashMap::new(),
            requested: HashSet::new(),
            artist_filter: None,
            genre_filter: None,
            rows: Vec::new(),
            shown: Vec::new(),
            view_dirty: true,
            stats: Stats::default(),
            stats_dirty: true,
            search: String::new(),
            search_results: Vec::new(),
            searched: String::new(),
            toast: None,
            welcome: first_start.then_some(Welcome { message: None }),
            sort_open: false,
            theme_open: false,
            hover_theme: None,
            focus_search: false,
            lyrics: None,
            lyrics_menu_open: false,
            editing: None,
            tidy: None,
            deleting: None,
            duplicates: None,
            dup_groups: HashMap::new(),
            paused: Arc::default(),
            wiki: Arc::new(std::sync::Mutex::new(crate::wiki::Cache::load(&data))),
            wiki_line: Arc::default(),
            wiki_started: false,
            wiki_waiting: 0,
            wiki_offline: false,
            wiki_open: false,
            wiki_list: Vec::new(),
            wiki_ticks: HashMap::new(),
            wiki_fill: None,
            downloads: Downloads { tools: crate::downloads::Tools::find(&data), ..Default::default() },
            import_pending: None,
            wiki_saved_at: Instant::now(),
            busy: false,
            loudness: crate::loudness::load(&data),
            loudness_changed: None,
            meter,
            spectrum: Spectrum::default(),
            eq_moving: false,
            backdrop: None,
            old_backdrop: None,
            events,
            send,
            media,
            media_shown: (false, Instant::now()),
            ctx,
            data,
        };
        if let Some(problem) = app.player.problem.clone() {
            app.show_toast(format!("{problem} Songs can't play until one is connected."));
        }
        app.start_wiki_worker();
        app.open_starting_folder();
        debug("app: ready");
        app
    }

    // ---------- The folder ----------

    // The default folder if it's there, otherwise your Music folder
    pub fn open_starting_folder(&mut self) {
        let default = store::default_folder(&self.data);
        let folder = match &default {
            Some(folder) if folder.is_dir() => folder.clone(),
            _ => {
                self.folder_missing = default.clone();
                let music = store::music_folder();
                let _ = std::fs::create_dir_all(&music);
                music
            }
        };
        if let Some(missing) = &self.folder_missing {
            self.show_toast(format!("Your default folder isn't there right now ({}), so your Music folder is open.", missing.display()));
        }
        self.open_folder(folder);
    }

    pub fn open_folder(&mut self, folder: PathBuf) {
        let mine = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.folder = folder.clone();
        self.songs.clear();
        self.by_id.clear();
        self.listed = false;
        self.artist_filter = None;
        self.genre_filter = None;
        self.view_dirty = true;
        self.stats_dirty = true;
        let ctx = self.ctx.clone();
        library::watch(folder, self.generation.clone(), mine, self.paused.clone(), self.send.clone(), move || ctx.request_repaint());
    }

    pub fn choose_default_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new().set_title("Pick your music folder").set_directory(&self.folder).pick_folder() else {
            return;
        };
        store::set_default_folder(&self.data, &folder);
        self.folder_missing = None;
        self.show_toast(format!("{} is your default folder now: it opens every time.", folder.display()));
        self.open_folder(folder);
    }

    // The folder changed: songs that are still there stay as they are (with their tags),
    // new ones are added and their tags read, removed ones go away
    fn take_listing(&mut self, listing: Listing) {
        let mut old: HashMap<String, Song> = self.songs.drain(..).map(|song| (format!("{}|{}", song.rel, song.size), song)).collect();
        let mut new_songs = Vec::new();
        let mut added = 0;
        for entry in &listing.songs {
            let song = match old.remove(&format!("{}|{}", entry.rel, entry.size)) {
                Some(song) => song,
                None => {
                    added += 1;
                    let mut song = Song::new(entry, &self.folder);
                    song.apply_record(self.library.get(&song.id));
                    new_songs.push((song.id.clone(), song.path.clone()));
                    song
                }
            };
            self.songs.push(song);
        }
        let removed = old.len();
        debug(&format!("listing: {} files, {} songs now, {added} added, {removed} removed", listing.songs.len(), self.songs.len()));
        self.folder_covers = listing.folder_covers;
        self.reindex();
        if self.listed && (added > 0 || removed > 0) {
            let mut parts = Vec::new();
            if added > 0 {
                parts.push(format!("{} added", plural(added, "new song")));
            }
            if removed > 0 {
                parts.push(format!("{} removed", plural(removed, "song")));
            }
            self.show_toast(format!("{}.", capitalize(&parts.join(", "))));
        }
        self.listed = true;
        for (id, path) in &new_songs {
            if !self.loudness.contains_key(id) {
                self.meter.request(id.clone(), path.clone(), false);
            }
        }
        if !new_songs.is_empty() {
            self.tags_waiting += new_songs.len();
            let ctx = self.ctx.clone();
            library::read_tags_in_background(new_songs, self.send.clone(), move || ctx.request_repaint());
        }
    }

    fn reindex(&mut self) {
        self.by_id = self.songs.iter().enumerate().map(|(i, song)| (song.id.clone(), i)).collect();
        self.view_dirty = true;
        self.stats_dirty = true;
        self.searched.clear();
    }

    pub fn song(&self, id: &str) -> Option<&Song> {
        self.by_id.get(id).map(|&i| &self.songs[i])
    }

    pub fn current_song(&self) -> Option<&Song> {
        self.current.as_deref().and_then(|id| self.song(id))
    }

    // ---------- Every frame ----------

    pub fn handle_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Listing(generation, listing) => {
                    if generation == self.generation.load(Ordering::Relaxed) {
                        self.take_listing(listing);
                    }
                }
                Event::Tags(result) => {
                    self.tags_waiting = self.tags_waiting.saturating_sub(1);
                    if let Some(&i) = self.by_id.get(&result.id) {
                        let song = &mut self.songs[i];
                        song.tags = result.tags;
                        song.duration = result.duration;
                        song.bitrate = result.bitrate;
                        song.tags_read = true;
                        song.natural = names::natural_names(&song.tags, &song.rel);
                        song.apply_record(self.library.get(&song.id));
                        library::remember_song(&mut self.library, &self.songs[i]);
                        if self.current.as_deref() == Some(result.id.as_str()) && self.duration <= 0.0 {
                            self.duration = result.duration;
                        }
                        self.view_dirty = true;
                        self.stats_dirty = true;
                        self.searched.clear();
                    }
                }
                Event::Cover(cover) => {
                    let options = egui::TextureOptions::LINEAR;
                    let entry = self.textures.entry(cover.id.clone()).or_default();
                    entry.none = cover.big.is_none() && cover.small.is_none();
                    if let Some(image) = cover.big {
                        entry.big = Some(self.ctx.load_texture(format!("{}-big", cover.id), image, options));
                    }
                    if let Some(image) = cover.backdrop {
                        entry.backdrop = Some(self.ctx.load_texture(format!("{}-backdrop", cover.id), image, options));
                    }
                    if let Some(image) = cover.small {
                        entry.small = Some(self.ctx.load_texture(format!("{}-small", cover.id), image, options));
                    }
                }
                Event::Media(command) => match command {
                    Command::Toggle => self.toggle_play(),
                    Command::Play => self.player.play(),
                    Command::Pause => self.player.pause(),
                    Command::Next => self.play_next(),
                    Command::Previous => self.play_previous(),
                    Command::SeekTo(seconds) => self.player.seek(seconds),
                    Command::SeekBy(seconds) => self.player.seek(self.player.position() + seconds),
                    Command::Raise => self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus),
                },
                Event::Tidied(done) => self.finish_tidy(done),
                Event::ToolsReady(result) => {
                    self.downloads.getting_tools = false;
                    self.downloads.tools = crate::downloads::Tools::find(&self.data);
                    match result {
                        Ok(()) => self.show_toast("Ready to download.".into()),
                        Err(error) => self.downloads.error = Some(format!("Couldn't get the download tools: {error}")),
                    }
                }
                Event::Checked(result) => {
                    self.downloads.checking = false;
                    match result {
                        Ok((playlist, entries)) => {
                            self.downloads.error = entries.is_empty().then(|| "No songs found at that link.".to_string());
                            self.downloads.playlist = playlist;
                            self.downloads.entries = entries;
                        }
                        Err(error) => self.downloads.error = Some(error),
                    }
                }
                Event::Download(job, state) => {
                    if let crate::downloads::State::Done(name) = &state {
                        if self.downloads.active() <= 1 {
                            self.show_toast(format!("Downloaded \"{name}\". It shows up in the list in a moment."));
                        }
                    }
                    if let Some(entry) = self.downloads.jobs.get_mut(job) {
                        entry.1 = state;
                    }
                }
                Event::Wiki(key, reached) => {
                    self.wiki_waiting = self.wiki_waiting.saturating_sub(1);
                    self.wiki_offline |= !reached;
                    if self.wiki_fill.as_deref() == Some(key.as_str()) {
                        self.wiki_fill = None;
                        self.fill_from_wiki(&key);
                    }
                    self.refresh_wiki_list();
                }
                Event::Lyrics(id, found) => {
                    if let Some(view) = self.lyrics.as_mut().filter(|view| view.id == id) {
                        view.lines = found.synced.as_deref().map(lyrics::parse_lrc).unwrap_or_default();
                        view.found = Some(found);
                        view.offset = 0.0;
                        view.scrolled_at = None;
                    }
                }
                Event::Loudness(id, measured) => {
                    self.loudness.insert(id.clone(), measured);
                    self.loudness_changed.get_or_insert_with(Instant::now);
                    if self.current.as_deref() == Some(id.as_str()) {
                        self.apply_even_volume();
                    }
                }
            }
        }
    }

    pub fn tick(&mut self) {
        // The song ended: the next one plays (unless it was the last and shuffle is off)
        if self.player.just_finished() {
            let last = self.queue.last() == self.current.as_ref();
            if !last || self.prefs.shuffle {
                self.play_next();
            }
        }
        // A play counts (for "Most played") after 30 seconds, or half of a shorter song
        if let Some(id) = self.current.clone() {
            if self.counted.as_ref() != Some(&id) && self.duration > 0.0 && self.player.position() >= (self.duration / 2.0).min(30.0) {
                self.counted = Some(id.clone());
                self.library.change(&id, |record| record.plays += 1);
                if self.prefs.sort == "plays" {
                    self.view_dirty = true;
                }
            }
        }
        // The desktop's media controls show what's happening
        let playing = self.player.is_playing();
        if playing != self.media_shown.0 || self.media_shown.1.elapsed() > Duration::from_secs(5) {
            self.media_shown = (playing, Instant::now());
            self.media.show_state(playing, self.player.position());
        }
        if !self.wiki_started && self.listed && self.tags_waiting == 0 {
            self.wiki_started = true;
            self.queue_wiki_lookups();
        }
        if self.wiki_saved_at.elapsed() > Duration::from_secs(20) {
            self.wiki_saved_at = Instant::now();
            self.wiki.lock().unwrap().save(&self.data);
        }
        // The equalizer
        let playing_now = self.player.is_playing();
        self.eq_moving = self.spectrum.update(&self.player.recent, playing_now);
        // The cover behind everything: the playing song's, fading in over the old one
        if let Some(id) = self.current.clone() {
            let ready = self.textures.get(&id).and_then(|c| c.backdrop.clone());
            if let Some(texture) = ready.filter(|_| self.backdrop.as_ref().is_none_or(|(shown, _)| *shown != id)) {
                if let Some((_, old)) = self.backdrop.take() {
                    self.old_backdrop = Some((old, Instant::now()));
                }
                self.backdrop = Some((id, texture));
            }
        }
        if let Some(changed) = self.loudness_changed {
            if changed.elapsed() > Duration::from_secs(20) {
                crate::loudness::save(&self.data, &self.loudness);
                self.loudness_changed = None;
            }
        }
        if self.prefs != self.saved_prefs {
            self.prefs.save(&self.data);
            self.saved_prefs = self.prefs.clone();
        }
        self.library.save_if_needed(false);
        if self.view_dirty {
            self.rebuild_view();
        }
        if self.stats_dirty {
            self.rebuild_stats();
        }
        if self.searched != self.search {
            self.run_search();
        }
    }

    pub fn save_all(&mut self) {
        self.library.save_if_needed(true);
        if self.loudness_changed.is_some() {
            crate::loudness::save(&self.data, &self.loudness);
        }
        self.wiki.lock().unwrap().save(&self.data);
        crate::downloads::cancel(&self.downloads.running);
        self.prefs.save(&self.data);
    }

    // ---------- Playing ----------

    // Starts a song from a list. The list becomes what plays next: in its own order, or in
    // a random order (starting with this song) while shuffle is on.
    pub fn play_from_list(&mut self, id: &str, list: Vec<String>) {
        self.ordered = list.clone();
        self.queue = if self.prefs.shuffle {
            let mut rest: Vec<String> = list.into_iter().filter(|other| other != id).collect();
            fastrand::shuffle(&mut rest);
            std::iter::once(id.to_string()).chain(rest).collect()
        } else {
            list
        };
        self.play(id);
    }

    pub fn play(&mut self, id: &str) {
        let Some(song) = self.song(id) else { return };
        let (path, title, artist, album, duration) = (song.path.clone(), song.title.clone(), song.artist.clone(), song.album.clone(), song.duration);
        match self.player.load(&path) {
            Ok(()) => {
                self.current = Some(id.to_string());
                self.counted = None;
                self.duration = duration;
                self.scroll_to_current = true;
                self.media.show_song(&title, &artist, &album, duration);
                self.request_cover(id, true);
                self.view_dirty = true; // the playing marks move
                self.apply_even_volume();
                if !self.loudness.contains_key(id) {
                    self.meter.request(id.to_string(), path.clone(), true);
                }
                if self.lyrics.as_ref().is_none_or(|view| view.id != id) {
                    self.load_lyrics(id, None);
                }
            }
            Err(error) => {
                self.show_toast(format!("Can't play \"{title}\": {error}"));
            }
        }
    }

    pub fn toggle_play(&mut self) {
        if self.current.is_none() || !self.player.has_song() {
            let list: Vec<String> = self.shown.iter().map(|&i| self.songs[i].id.clone()).collect();
            if let Some(first) = if self.prefs.shuffle { fastrand::choice(&list).cloned() } else { list.first().cloned() } {
                self.play_from_list(&first, list);
            }
        } else if self.player.is_playing() {
            self.player.pause();
        } else if self.player.ended() {
            if let Some(id) = self.current.clone() {
                self.play(&id); // played to the end: from the start again
            }
        } else {
            self.player.play();
        }
    }

    pub fn play_next(&mut self) {
        let index = self.current.as_ref().and_then(|id| self.queue.iter().position(|other| other == id));
        let next = match index {
            Some(i) if i + 1 < self.queue.len() => self.queue[i + 1].clone(),
            _ if self.prefs.shuffle && !self.ordered.is_empty() => {
                // Shuffle never runs out: it shuffles again
                let mut again = self.ordered.clone();
                fastrand::shuffle(&mut again);
                self.queue = again;
                self.queue[0].clone()
            }
            _ => match self.queue.first() {
                Some(first) => first.clone(),
                None => return self.toggle_play(),
            },
        };
        self.play(&next);
    }

    // Like most players: the first press goes back to the start of the song
    pub fn play_previous(&mut self) {
        let index = self.current.as_ref().and_then(|id| self.queue.iter().position(|other| other == id));
        match index {
            Some(i) if i > 0 && self.player.position() <= 3.0 => {
                let previous = self.queue[i - 1].clone();
                self.play(&previous);
            }
            _ => self.player.seek(0.0),
        }
    }

    // On: the songs play in random order, starting right away if nothing is playing.
    // Off: back to the list's own order, from the song that's playing.
    pub fn toggle_shuffle(&mut self) {
        self.prefs.shuffle = !self.prefs.shuffle;
        let list: Vec<String> = if self.ordered.is_empty() { self.shown.iter().map(|&i| self.songs[i].id.clone()).collect() } else { self.ordered.clone() };
        if !self.prefs.shuffle {
            if self.current.is_some() && !self.ordered.is_empty() {
                self.queue = self.ordered.clone();
            }
            self.show_toast("Shuffle off.".into());
            return;
        }
        if list.is_empty() {
            self.show_toast("Open a folder with music first, then shuffle.".into());
            return;
        }
        match self.current.clone() {
            Some(current) if self.player.is_playing() => {
                let mut rest: Vec<String> = list.iter().filter(|id| **id != current).cloned().collect();
                fastrand::shuffle(&mut rest);
                self.ordered = list;
                self.queue = std::iter::once(current).chain(rest).collect();
                self.show_toast("Shuffle on. The next songs come in random order.".into());
            }
            _ => {
                let mut random = list.clone();
                fastrand::shuffle(&mut random);
                self.ordered = list;
                self.queue = random;
                let first = self.queue[0].clone();
                self.play(&first);
                self.show_toast("Shuffle on.".into());
            }
        }
    }

    // ---------- Changing songs (the pencil, Tidy file names, deleting) ----------

    pub fn duplicate_group(&self, id: &str) -> Option<&Vec<String>> {
        let song = self.song(id)?;
        self.dup_groups.get(&format!("{}|{}", names::simplify(&song.artist), names::simplify(&song.title)))
    }

    fn file_name_of(song: &Song) -> String {
        song.rel.rsplit('/').next().unwrap_or(&song.rel).to_string()
    }

    fn taggable(song: &Song) -> bool {
        let extension = song.rel.rsplit('.').next().unwrap_or("").to_lowercase();
        ["mp3", "m4a", "flac", "ogg", "oga", "opus", "wav"].contains(&extension.as_str())
    }

    // Names you gave a song that its file doesn't have yet: (in its tags, in its file name)
    pub fn unsaved_names(&self, song: &Song) -> (bool, String) {
        let edited = self.library.get(&song.id).map_or(&[][..], |r| r.edited.as_slice());
        if edited.is_empty() || !song.tags_read {
            return (false, String::new());
        }
        let shown = [song.title.as_str(), if song.known_artist() { song.artist.as_str() } else { "" }, song.album.as_str(), song.genre.as_str()];
        let tags = [&song.tags.title, &song.tags.artist, &song.tags.album, &song.tags.genre];
        let tags_differ = Self::taggable(song) && shown.iter().zip(tags).any(|(a, b)| a.trim() != b.trim());
        let rename = if edited.iter().any(|f| f == "title" || f == "artist") { crate::edit::file_name_for(&Self::file_name_of(song), &song.title, &song.artist) } else { String::new() };
        (tags_differ, rename)
    }

    pub fn start_editing(&mut self, id: &str) {
        let Some(song) = self.song(id) else { return };
        self.editing = Some(EditForm {
            id: id.to_string(),
            title: song.title.clone(),
            artist: if song.known_artist() { song.artist.clone() } else { String::new() },
            album: song.album.clone(),
            genre: song.genre.clone(),
            rename: true,
        });
    }

    // The file name the pencil's window offers (empty: none to offer)
    pub fn rename_offer(&self, form: &EditForm) -> String {
        let Some(song) = self.song(&form.id) else { return String::new() };
        let artist = if form.artist.trim().is_empty() { UNKNOWN_ARTIST } else { form.artist.trim() };
        if form.title.trim().is_empty() {
            return String::new(); // (every file is named "Artist - Title": offered whenever it isn't yet)
        }
        let new_name = crate::edit::file_name_for(&Self::file_name_of(song), form.title.trim(), artist);
        let taken = !new_name.is_empty() && {
            let new_rel = match song.rel.rsplit_once('/') { Some((folder, _)) => format!("{folder}/{new_name}"), None => new_name.clone() };
            new_rel.to_lowercase() != song.rel.to_lowercase() && self.folder.join(&new_rel).exists()
        };
        if taken { String::new() } else { new_name }
    }

    // The pencil's Save: library.json, the file's tags, and (if ticked) its name
    pub fn save_edit(&mut self) {
        let Some(form) = self.editing.take() else { return };
        let Some(song) = self.song(&form.id) else { return };
        let artist_shown = if form.artist.trim().is_empty() { UNKNOWN_ARTIST.to_string() } else { form.artist.trim().to_string() };
        let genres = names::clean_genres(&form.genre);
        let wanted = names::Names { title: form.title.trim().to_string(), artist: artist_shown.clone(), album: form.album.trim().to_string(), genre: genres.join(", ") };
        // Which names differ from the file's own (they're "yours" from now on)
        let natural = song.natural.clone();
        let mut changed: Vec<&str> = Vec::new();
        if wanted.title != natural.title { changed.push("title"); }
        if wanted.artist != natural.artist { changed.push("artist"); }
        if wanted.album != natural.album { changed.push("album"); }
        if wanted.genre != natural.genre { changed.push("genres"); }
        let rename_to = if form.rename { self.rename_offer(&form) } else { String::new() };
        let id = form.id.clone();
        self.library.change(&id, |record| {
            record.title = Some(wanted.title.clone());
            record.artist = Some(if wanted.artist == UNKNOWN_ARTIST { String::new() } else { wanted.artist.clone() });
            record.album = Some(wanted.album.clone());
            record.genres = Some(genres.clone());
            for field in &changed {
                if !record.edited.iter().any(|f| f == field) {
                    record.edited.push(field.to_string());
                }
            }
        });
        let message = self.save_to_file(&id, &wanted, &rename_to);
        self.show_toast(message);
    }

    // Writes names into a song's tags and renames its file; the song keeps everything
    // (record, cover, place in the queue) under its new name and size. Says what happened.
    fn save_to_file(&mut self, id: &str, wanted: &names::Names, rename_to: &str) -> String {
        let Some(song) = self.song(id) else { return "That song isn't here any more.".into() };
        let (path, old_rel, modified, taggable) = (song.path.clone(), song.rel.clone(), song.modified, Self::taggable(song));
        let playing_here = self.current.as_deref() == Some(id);
        let (position, was_playing) = (self.player.position(), self.player.is_playing());
        let mut parts = vec!["Names saved".to_string()];
        let mut problems = Vec::new();
        let mut tags_written = false;
        if taggable {
            match crate::edit::write_tags(&path, wanted) {
                Ok(()) => {
                    parts[0] = "Names saved, also inside the file".into();
                    tags_written = true;
                }
                Err(error) => problems.push(format!("They couldn't be written inside the file ({error}).")),
            }
        }
        let mut new_rel = old_rel.clone();
        if !rename_to.is_empty() {
            let target = match old_rel.rsplit_once('/') { Some((folder, _)) => format!("{folder}/{rename_to}"), None => rename_to.to_string() };
            match crate::edit::rename(&self.folder, &old_rel, &target) {
                Ok(()) => {
                    crate::edit::save_undo(&self.data, &self.folder, &[(old_rel.clone(), target.clone())]);
                    parts.push(format!("the file renamed to \"{rename_to}\""));
                    new_rel = target;
                }
                Err(error) => problems.push(format!("The file couldn't be renamed ({error}).")),
            }
        }
        let size = std::fs::metadata(self.folder.join(&new_rel)).map_or(0, |m| m.len());
        self.move_song(id, &new_rel, size, modified);
        if tags_written {
            let new_id = self.song_id(&new_rel, size);
            if let Some(&i) = self.by_id.get(&new_id) {
                let song = &mut self.songs[i];
                song.tags = names::Names { artist: if wanted.artist == UNKNOWN_ARTIST { String::new() } else { wanted.artist.clone() }, ..wanted.clone() };
                song.natural = names::natural_names(&song.tags, &song.rel);
                song.apply_record(self.library.get(&new_id));
            }
        }
        // The song playing was changed on disk: it continues from the same moment
        if playing_here {
            if let Some(song) = self.current_song() {
                let path = song.path.clone();
                if self.player.load(&path).is_ok() {
                    self.player.seek(position);
                    if !was_playing {
                        self.player.pause();
                    }
                }
            }
        }
        let mut message = format!("{}.", parts.join(", and "));
        for problem in problems {
            message.push(' ');
            message.push_str(&problem);
        }
        message
    }

    fn song_id(&self, rel: &str, size: u64) -> String {
        format!("{}|{size}", rel.rsplit('/').next().unwrap_or(rel))
    }

    // A song's file got a new name or size: its id ("name|size") changes, and its record,
    // cover, loudness and place in the queue move along
    fn move_song(&mut self, old_id: &str, new_rel: &str, size: u64, modified: u64) {
        let new_id = self.song_id(new_rel, size);
        let Some(&i) = self.by_id.get(old_id) else { return };
        if new_id != old_id {
            if let Some(mut record) = self.library.songs.shift_remove(old_id) {
                record.file = Some(new_rel.to_string());
                record.added.get_or_insert(modified); // the file looks new now; "Recently added" keeps its date
                self.library.songs.insert(new_id.clone(), record);
                self.library.change(&new_id, |_| {});
                self.library.mark_changed();
            }
            if let Some(measured) = self.loudness.remove(old_id) {
                self.loudness.insert(new_id.clone(), measured);
                self.loudness_changed.get_or_insert_with(Instant::now);
            }
            if let Some(textures) = self.textures.remove(old_id) {
                self.textures.insert(new_id.clone(), textures);
            }
            for list in [&mut self.queue, &mut self.ordered] {
                for id in list.iter_mut().filter(|id| *id == old_id) {
                    *id = new_id.clone();
                }
            }
            if self.current.as_deref() == Some(old_id) {
                self.current = Some(new_id.clone());
            }
            if let Some(view) = self.lyrics.as_mut().filter(|v| v.id == old_id) {
                view.id = new_id.clone();
            }
        }
        let song = &mut self.songs[i];
        song.id = new_id;
        song.rel = new_rel.to_string();
        song.path = self.folder.join(new_rel);
        song.size = size;
        song.apply_record(self.library.get(&song.id));
        self.reindex();
    }

    // Puts the file's own names back (the pencil's "Use the original names")
    pub fn use_original_names(&mut self, id: &str) {
        self.editing = None;
        self.library.change(id, |record| {
            record.title = None;
            record.artist = None;
            record.album = None;
            record.genres = None;
            record.edited.clear();
        });
        if let Some(&i) = self.by_id.get(id) {
            library::remember_song(&mut self.library, &self.songs[i]);
            self.songs[i].apply_record(self.library.get(id));
        }
        self.reindex();
        self.show_toast("Back to the file's own names.".into());
    }

    // "Tidy file names": tidier names for files with extras, and the names you gave songs
    // for files that don't have them yet
    pub fn open_tidy(&mut self) {
        let mut taken: HashSet<String> = self.songs.iter().map(|s| s.rel.to_lowercase()).collect();
        let mut list = Vec::new();
        for song in &self.songs {
            let old_name = Self::file_name_of(song);
            let (tags, _) = self.unsaved_names(song);
            let new_name = crate::edit::file_name_for(&old_name, &song.title, &song.artist);
            let new_name = if new_name.is_empty() { old_name.clone() } else { new_name };
            if new_name == old_name && !tags {
                continue;
            }
            let new_rel = match song.rel.rsplit_once('/') { Some((folder, _)) => format!("{folder}/{new_name}"), None => new_name.clone() };
            let clash = new_name != old_name && new_rel.to_lowercase() != song.rel.to_lowercase() && taken.contains(&new_rel.to_lowercase());
            if !clash {
                taken.insert(new_rel.to_lowercase());
            }
            list.push(Tidy { id: song.id.clone(), old_name, new_name, tags, clash, chosen: !clash || tags });
        }
        self.tidy = Some(list);
    }

    // The files are changed in the background (writing tags can take a while for many
    // songs); folder watching waits meanwhile, and the songs are updated when it's done
    pub fn apply_tidy(&mut self) {
        let Some(list) = self.tidy.take() else { return };
        let mut jobs = Vec::new();
        for item in list.into_iter().filter(|item| item.chosen) {
            let Some(song) = self.song(&item.id) else { continue };
            let shown = names::Names { title: song.title.clone(), artist: song.artist.clone(), album: song.album.clone(), genre: song.genre.clone() };
            let target = if item.clash || item.new_name == item.old_name {
                song.rel.clone()
            } else {
                match song.rel.rsplit_once('/') { Some((folder, _)) => format!("{folder}/{}", item.new_name), None => item.new_name.clone() }
            };
            jobs.push((item.id.clone(), song.rel.clone(), target, song.modified, item.tags.then_some(shown)));
        }
        if jobs.is_empty() {
            return;
        }
        self.busy = true;
        self.paused.store(true, Ordering::Relaxed);
        if jobs.len() > 5 {
            self.show_toast(format!("Changing {}…", plural(jobs.len(), "file")));
        }
        let (folder, send, ctx) = (self.folder.clone(), self.send.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let mut done = Vec::new();
            for (id, old_rel, target, modified, tags) in jobs {
                let mut failed = false;
                let tags = tags.filter(|names| {
                    let ok = crate::edit::write_tags(&folder.join(&old_rel), names).is_ok();
                    failed |= !ok;
                    ok
                });
                let mut new_rel = old_rel.clone();
                if target != old_rel {
                    match crate::edit::rename(&folder, &old_rel, &target) {
                        Ok(()) => new_rel = target,
                        Err(_) => failed = true,
                    }
                }
                done.push(Done { id, old_rel, new_rel, modified, tags, failed });
            }
            let _ = send.send(Event::Tidied(done));
            ctx.request_repaint();
        });
    }

    // The background tidying is done: the songs follow their files
    fn finish_tidy(&mut self, done: Vec<Done>) {
        let (mut renamed, mut tagged, mut failed) = (Vec::new(), 0, 0);
        let playing = self.current.clone();
        let mut reload = None;
        for item in done {
            failed += usize::from(item.failed);
            let size = std::fs::metadata(self.folder.join(&item.new_rel)).map_or(0, |m| m.len());
            if item.new_rel != item.old_rel {
                renamed.push((item.old_rel.clone(), item.new_rel.clone()));
            }
            if item.tags.is_none() && item.new_rel == item.old_rel {
                continue;
            }
            let was_playing = playing.as_deref() == Some(item.id.as_str());
            self.move_song(&item.id, &item.new_rel, size, item.modified);
            let new_id = self.song_id(&item.new_rel, size);
            if let Some(shown) = item.tags {
                tagged += 1;
                if let Some(&i) = self.by_id.get(&new_id) {
                    let song = &mut self.songs[i];
                    song.tags = names::Names { artist: if shown.artist == UNKNOWN_ARTIST { String::new() } else { shown.artist.clone() }, ..shown };
                    song.natural = names::natural_names(&song.tags, &song.rel);
                    song.apply_record(self.library.get(&new_id));
                }
                if was_playing {
                    reload = Some(new_id.clone()); // its file was rewritten: it goes on from the same moment
                }
            }
        }
        if let Some(id) = reload {
            self.reload_current(&id);
        }
        if !renamed.is_empty() {
            crate::edit::save_undo(&self.data, &self.folder, &renamed);
        }
        self.paused.store(false, Ordering::Relaxed);
        self.busy = false;
        self.reindex();
        let mut parts = Vec::new();
        if !renamed.is_empty() { parts.push(format!("{} renamed", plural(renamed.len(), "file"))); }
        if tagged > 0 { parts.push(format!("names saved inside {}", plural(tagged, "file"))); }
        let mut message = if parts.is_empty() { "Nothing was changed.".to_string() } else { capitalize(&format!("{}.", parts.join(", "))) };
        if failed > 0 {
            message.push_str(&format!(" {} couldn't be changed (in use, or the name is taken).", plural(failed, "file")));
        }
        self.show_toast(message);
    }

    // The playing song's file changed: it goes on from the same moment (paused if it was)
    fn reload_current(&mut self, id: &str) {
        let Some(song) = self.song(id) else { return };
        let (path, position, playing) = (song.path.clone(), self.player.position(), self.player.is_playing());
        if self.player.load(&path).is_ok() {
            self.player.seek(position);
            if !playing {
                self.player.pause();
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !crate::edit::load_undo(&self.data, &self.folder).is_empty()
    }

    // Puts the last renamed files back under their old names
    pub fn undo_rename(&mut self) {
        let renames = crate::edit::load_undo(&self.data, &self.folder);
        let mut done = 0;
        for (from, to) in renames.iter().rev() {
            let Some(song) = self.songs.iter().find(|s| s.rel == *to) else { continue };
            let (id, modified) = (song.id.clone(), song.modified);
            if crate::edit::rename(&self.folder, to, from).is_ok() {
                let size = std::fs::metadata(self.folder.join(from)).map_or(0, |m| m.len());
                self.move_song(&id, from, size, modified);
                done += 1;
            }
        }
        crate::edit::clear_undo(&self.data);
        self.tidy = None;
        self.show_toast(format!("{} put back.", plural(done, "file name")));
    }

    // Deleting: the file goes to the Trash, and its record goes away
    pub fn delete_song(&mut self, id: &str) {
        self.deleting = None;
        let Some(song) = self.song(id) else { return };
        let (path, title) = (song.path.clone(), song.title.clone());
        if self.current.as_deref() == Some(id) {
            // The next song plays if music was playing; otherwise nothing is left loaded
            let was_playing = self.player.is_playing();
            if was_playing && self.queue.len() > 1 {
                self.play_next();
            } else {
                self.player.pause();
                self.current = None;
                self.lyrics = None;
            }
        }
        match crate::edit::move_to_trash(&path) {
            Ok(()) => {
                self.queue.retain(|other| other != id);
                self.ordered.retain(|other| other != id);
                self.library.songs.shift_remove(id);
                self.library.mark_changed();
                self.songs.retain(|song| song.id != id);
                self.reindex();
                self.show_toast(format!("\"{title}\" is in the Trash."));
            }
            Err(error) => self.show_toast(format!("Couldn't delete \"{title}\": {error}")),
        }
    }

    // Copies of the song: the best first (lossless, then the highest bitrate, then the
    // biggest file). Copies much longer or shorter are probably other versions (a live
    // one, an edit), so they aren't ticked for deleting.
    pub fn open_duplicates(&mut self, id: &str) {
        let Some(group) = self.duplicate_group(id) else { return };
        let mut ids = group.clone();
        let quality = |id: &String| self.song(id).map(|s| crate::edit::quality(&s.rel.rsplit('.').next().unwrap_or("").to_lowercase(), s.bitrate, s.size));
        ids.sort_by(|a, b| quality(b).cmp(&quality(a)));
        let best = self.song(&ids[0]).map_or(0.0, |s| s.duration);
        let remove = ids.iter().enumerate().map(|(i, id)| i > 0 && self.song(id).is_some_and(|s| (s.duration - best).abs() <= 10.0)).collect();
        self.duplicates = Some(Duplicates { ids, remove });
    }

    // Deletes the ticked copies; what they had (heart, plays, lyrics choice, album and
    // genres) goes to the copy that stays
    pub fn remove_duplicates(&mut self) {
        let Some(window) = self.duplicates.take() else { return };
        let keep = window.ids.iter().zip(&window.remove).find(|(_, remove)| !**remove).map(|(id, _)| id.clone());
        let Some(keep) = keep else { return };
        let mut removed = 0;
        for (id, _) in window.ids.iter().zip(&window.remove).filter(|(_, remove)| **remove) {
            if let Some(gone) = self.library.get(id).cloned() {
                self.library.change(&keep, |record| {
                    if record.favorite.is_none() { record.favorite = gone.favorite.clone(); }
                    record.plays += gone.plays;
                    if record.lyrics.is_none() { record.lyrics = gone.lyrics.clone(); }
                    if record.album.as_deref().is_none_or(str::is_empty) { record.album = gone.album.clone(); }
                    if record.genres.as_ref().is_none_or(Vec::is_empty) { record.genres = gone.genres.clone(); }
                });
            }
            let was_current = self.current.as_deref() == Some(id.as_str());
            let (position, playing) = (self.player.position(), self.player.is_playing());
            self.delete_song(id);
            if was_current {
                // The copy that stays plays on from the same moment
                self.play(&keep);
                self.player.seek(position);
                if !playing { self.player.pause(); }
            }
            removed += 1;
        }
        if let Some(&i) = self.by_id.get(&keep) {
            self.songs[i].apply_record(self.library.get(&keep));
        }
        self.show_toast(format!("{} in the Trash; the best copy stays.", plural(removed, "copy")));
    }

    // ---------- Downloads ----------

    pub fn open_downloads(&mut self) {
        self.downloads.open = true;
        self.downloads.tools = crate::downloads::Tools::find(&self.data);
        if !self.downloads.updated && self.downloads.tools.ready() {
            self.downloads.updated = true;
            let tools = self.downloads.tools.clone();
            std::thread::spawn(move || crate::downloads::update_yt_dlp(&tools)); // (once a day at most)
        }
    }

    pub fn get_download_tools(&mut self) {
        self.downloads.getting_tools = true;
        self.downloads.error = None;
        let (tools, send, ctx) = (self.downloads.tools.clone(), self.send.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = send.send(Event::ToolsReady(crate::downloads::get_tools(&tools)));
            ctx.request_repaint();
        });
    }

    pub fn check_link(&mut self) {
        let link = self.downloads.link.trim().to_string();
        if link.is_empty() {
            return;
        }
        self.downloads.checking = true;
        self.downloads.error = None;
        self.downloads.entries.clear();
        let (tools, send, ctx) = (self.downloads.tools.clone(), self.send.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = send.send(Event::Checked(crate::downloads::check(&tools, &link)));
            ctx.request_repaint();
        });
    }

    // The ticked songs go in line; up to three download at the same time
    pub fn start_downloads(&mut self) {
        let chosen: Vec<crate::downloads::Entry> = self.downloads.entries.drain(..).filter(|e| e.chosen && !e.title.trim().is_empty()).collect();
        self.downloads.link.clear();
        self.downloads.playlist.clear();
        {
            let mut queue = self.downloads.queue.lock().unwrap();
            for entry in chosen {
                queue.push_back((self.downloads.jobs.len(), entry.clone()));
                self.downloads.jobs.push((entry, crate::downloads::State::Waiting));
            }
        }
        let waiting = self.downloads.queue.lock().unwrap().len();
        while self.downloads.workers.load(Ordering::Relaxed) < crate::downloads::PARALLEL.min(waiting) {
            self.downloads.workers.fetch_add(1, Ordering::Relaxed);
            let (queue, workers, running) = (self.downloads.queue.clone(), self.downloads.workers.clone(), self.downloads.running.clone());
            let (tools, folder, send, ctx) = (self.downloads.tools.clone(), self.folder.clone(), self.send.clone(), self.ctx.clone());
            std::thread::spawn(move || loop {
                let next = queue.lock().unwrap().pop_front();
                let Some((job, entry)) = next else {
                    workers.fetch_sub(1, Ordering::Relaxed);
                    return;
                };
                let tell = |state: crate::downloads::State| {
                    let _ = send.send(Event::Download(job, state));
                    ctx.request_repaint();
                };
                tell(crate::downloads::State::Downloading(0.0));
                let result = crate::downloads::download(&tools, &folder, job, &entry, &running, &tell);
                tell(result);
            });
        }
    }

    // Stops everything: waiting songs aren't downloaded, half-downloaded ones go away
    pub fn cancel_downloads(&mut self) {
        let waiting: Vec<usize> = self.downloads.queue.lock().unwrap().drain(..).map(|(job, _)| job).collect();
        for job in waiting {
            if let Some(entry) = self.downloads.jobs.get_mut(job) {
                entry.1 = crate::downloads::State::Cancelled;
            }
        }
        crate::downloads::cancel(&self.downloads.running);
    }

    pub fn clear_finished_downloads(&mut self) {
        if self.downloads.active() == 0 {
            self.downloads.jobs.clear();
        }
    }

    // ---------- Export and import ----------

    pub fn export_library(&mut self) {
        self.save_all();
        let name = format!("music-player-library-{}.zip", &store::now_iso()[..10]);
        let Some(to) = rfd::FileDialog::new().set_title("Export your library").set_file_name(&name).add_filter("Zip", &["zip"]).save_file() else { return };
        match crate::transfer::export(&self.data, &to) {
            Ok(count) => self.show_toast(format!("Exported {} into {}. Import it on any computer to bring everything along.", plural(count, "file"), to.display())),
            Err(error) => self.show_toast(format!("Couldn't export: {error}")),
        }
    }

    pub fn choose_import(&mut self) {
        if let Some(from) = rfd::FileDialog::new().set_title("Import a library").add_filter("Zip", &["zip"]).pick_file() {
            self.import_pending = Some(from);
        }
    }

    // Brings a .zip in, then everything is read again
    pub fn import_library(&mut self, from: &std::path::Path) {
        self.save_all();
        match crate::transfer::import(&self.data, from) {
            Ok(message) => {
                self.library = Library::load(&self.data);
                if let Some(prefs) = Preferences::load(&self.data) {
                    self.prefs = prefs;
                    self.theme = crate::theme::by_id(&self.prefs.theme);
                    self.player.set_volume(if self.prefs.muted { 0.0 } else { self.prefs.volume });
                }
                self.prefs.save(&self.data);
                self.loudness = crate::loudness::load(&self.data);
                *self.wiki.lock().unwrap() = crate::wiki::Cache::load(&self.data);
                self.textures.clear();
                self.requested.clear();
                self.welcome = None;
                self.wiki_started = false;
                self.open_starting_folder();
                self.show_toast(message);
            }
            Err(error) => {
                if let Some(welcome) = self.welcome.as_mut() {
                    welcome.message = Some(error);
                } else {
                    self.show_toast(format!("Couldn't import: {error}"));
                }
            }
        }
    }

    // ---------- Albums and genres from Wikipedia ----------

    // One worker asks Wikipedia about the songs in line, one after another
    fn start_wiki_worker(&mut self) {
        let (line, cache, send, ctx) = (self.wiki_line.clone(), self.wiki.clone(), self.send.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let mut asker = crate::wiki::Asker::default();
            loop {
                let (key, artist, title) = {
                    let (jobs, ready) = &*line;
                    let mut jobs = jobs.lock().unwrap();
                    loop {
                        if let Some(job) = jobs.pop_front() {
                            break job;
                        }
                        jobs = ready.wait(jobs).unwrap();
                    }
                };
                let reached = match asker.find(&cache, &artist, &title) {
                    Ok(found) => {
                        cache.lock().unwrap().put(&key, &found);
                        true
                    }
                    Err(error) => {
                        eprintln!("Wikipedia: {error}");
                        false
                    }
                };
                let _ = send.send(Event::Wiki(key, reached));
                ctx.request_repaint();
            }
        });
    }

    fn ask_wiki(&mut self, key: String, artist: String, title: String, urgent: bool) {
        let (jobs, ready) = &*self.wiki_line;
        let mut jobs = jobs.lock().unwrap();
        if urgent {
            jobs.retain(|(other, _, _)| *other != key);
            jobs.push_front((key, artist, title));
        } else if !jobs.iter().any(|(other, _, _)| *other == key) {
            jobs.push_back((key, artist, title));
        } else {
            return;
        }
        self.wiki_waiting += 1;
        ready.notify_one();
    }

    // Every song Wikipedia wasn't asked about yet (or not lately) goes in line
    pub fn queue_wiki_lookups(&mut self) {
        let mut wanted = Vec::new();
        {
            let cache = self.wiki.lock().unwrap();
            let mut seen = HashSet::new();
            for song in self.songs.iter().filter(|s| s.known_artist()) {
                let key = crate::wiki::key(&song.artist, &song.title);
                if cache.get(&key).is_none() && seen.insert(key.clone()) {
                    wanted.push((key, song.artist.clone(), song.title.clone()));
                }
            }
        }
        for (key, artist, title) in wanted {
            self.ask_wiki(key, artist, title, false);
        }
        self.refresh_wiki_list();
    }

    // What Wikipedia knows that the songs don't have: a missing album or genres (ticked
    // to use), or other ones than the songs have (not ticked). Names you set yourself
    // aren't suggested over.
    pub fn refresh_wiki_list(&mut self) {
        let cache = self.wiki.lock().unwrap();
        let mut list = Vec::new();
        for song in self.songs.iter().filter(|s| s.known_artist()) {
            let Some(found) = cache.get(&crate::wiki::key(&song.artist, &song.title)) else { continue };
            let edited = self.library.get(&song.id).map_or(&[][..], |r| r.edited.as_slice());
            let album = (!found.album.is_empty() && !edited.iter().any(|f| f == "album") && !names::same_name(&found.album, &song.album) || !found.album.is_empty() && song.album.is_empty())
                .then(|| (found.album.clone(), song.album.is_empty()));
            let mine: Vec<String> = names::genres_of(&song.genre).iter().map(|g| names::normalize(g)).collect();
            let theirs: Vec<String> = found.genres.iter().map(|g| names::normalize(g)).collect();
            let genres = (!found.genres.is_empty() && (mine.is_empty() || !edited.iter().any(|f| f == "genres") && mine != theirs))
                .then(|| (found.genres.clone(), mine.is_empty()));
            if album.is_some() || genres.is_some() {
                list.push(WikiSuggestion { id: song.id.clone(), album, genres });
            }
        }
        drop(cache);
        for item in &list {
            if let Some((_, missing)) = &item.album {
                self.wiki_ticks.entry((item.id.clone(), false)).or_insert(*missing);
            }
            if let Some((_, missing)) = &item.genres {
                self.wiki_ticks.entry((item.id.clone(), true)).or_insert(*missing);
            }
        }
        self.wiki_list = list;
    }

    // The ticked suggestions become the songs' album and genres (in library.json; Tidy
    // file names then offers to write them into the files)
    pub fn apply_wiki(&mut self) {
        let mut changed = 0;
        let list = std::mem::take(&mut self.wiki_list);
        for item in &list {
            let use_album = item.album.as_ref().filter(|_| self.wiki_ticks.get(&(item.id.clone(), false)) == Some(&true));
            let use_genres = item.genres.as_ref().filter(|_| self.wiki_ticks.get(&(item.id.clone(), true)) == Some(&true));
            if use_album.is_none() && use_genres.is_none() {
                continue;
            }
            changed += 1;
            self.library.change(&item.id, |record| {
                if let Some((album, _)) = use_album {
                    record.album = Some(album.clone());
                    if !record.edited.iter().any(|f| f == "album") {
                        record.edited.push("album".into());
                    }
                }
                if let Some((genres, _)) = use_genres {
                    record.genres = Some(genres.clone());
                    if !record.edited.iter().any(|f| f == "genres") {
                        record.edited.push("genres".into());
                    }
                }
            });
            if let Some(&i) = self.by_id.get(&item.id) {
                let song = &mut self.songs[i];
                song.apply_record(self.library.get(&item.id));
            }
        }
        self.wiki_ticks.clear();
        self.view_dirty = true;
        self.stats_dirty = true;
        self.refresh_wiki_list();
        self.wiki_open = false;
        self.show_toast(if changed > 0 { format!("Albums and genres from Wikipedia added to {}. Tidy file names can write them into the files.", plural(changed, "song")) } else { "Nothing was changed.".into() });
    }

    // The pencil's "Fill in from Wikipedia": what's known, or asked right away
    pub fn wiki_for_form(&mut self) {
        let Some(form) = &self.editing else { return };
        let artist = if form.artist.trim().is_empty() { return self.show_toast("Give the song an artist first.".into()) } else { form.artist.trim().to_string() };
        let key = crate::wiki::key(&artist, form.title.trim());
        let known = self.wiki.lock().unwrap().get(&key).is_some();
        if known {
            self.fill_from_wiki(&key);
        } else {
            self.wiki_fill = Some(key.clone());
            let title = form.title.trim().to_string();
            self.ask_wiki(key, artist, title, true);
        }
    }

    fn fill_from_wiki(&mut self, key: &str) {
        let found = self.wiki.lock().unwrap().get(key);
        let Some(form) = self.editing.as_mut() else { return };
        match found {
            Some(found) if !found.album.is_empty() || !found.genres.is_empty() => {
                if !found.album.is_empty() {
                    form.album = found.album.clone();
                }
                if !found.genres.is_empty() {
                    form.genre = found.genres.join(", ");
                }
                self.show_toast("Filled in from Wikipedia. Check them, then Save.".into());
            }
            _ => self.show_toast(if self.wiki_offline { "Couldn't reach Wikipedia. Check your internet connection.".into() } else { "Wikipedia doesn't know this song's album or genres.".into() }),
        }
    }

    // ---------- Even volume ----------

    pub fn apply_even_volume(&mut self) {
        let gain = match &self.current {
            Some(id) if self.prefs.even_volume => match self.loudness.get(id) {
                Some(measured) => crate::loudness::gain(measured),
                None => crate::loudness::typical_gain(&self.loudness), // not measured yet
            },
            _ => 1.0,
        };
        self.player.set_gain(gain);
    }

    pub fn toggle_even_volume(&mut self) {
        self.prefs.even_volume = !self.prefs.even_volume;
        self.apply_even_volume();
        self.show_toast(if self.prefs.even_volume { "Even volume on: loud songs are turned down, quiet ones up." } else { "Even volume off: songs play as they are." }.into());
    }

    // ---------- Lyrics ----------

    // Looks for a song's lyrics in the background. source: a source picked in the menu
    // (it becomes this song's choice), or None for the song's choice or the best match.
    pub fn load_lyrics(&mut self, id: &str, source: Option<&str>) {
        let Some(song) = self.song(id) else { return };
        let album_known = !song.album.is_empty() && (!song.tags.album.is_empty() || song.album != names::guess_from_file_name(&song.rel).album);
        let artist = if song.known_artist() { names::lookup_name(&song.artist) } else { String::new() };
        let query = lyrics::Query {
            path: song.path.clone(),
            lyrics_dir: self.data.join("lyrics"),
            file_name: lyrics::file_name(if song.known_artist() { &song.artist } else { "" }, &song.title),
            artist,
            title: song.title.clone(),
            album: if album_known { song.album.clone() } else { String::new() },
            duration: song.duration,
        };
        let choice = self.library.get(id).and_then(|record| record.lyrics.clone());
        self.lyrics = Some(LyricsView { id: id.to_string(), found: None, lines: Vec::new(), offset: 0.0, scrolled_at: None, tapping: None });
        let (send, ctx, id, picked) = (self.send.clone(), self.ctx.clone(), id.to_string(), source.map(str::to_string));
        std::thread::spawn(move || {
            let found = match &picked {
                Some(source) => match lyrics::from_source(&query, source) {
                    Ok(Some(found)) if found.has_lyrics() => found,
                    Ok(_) => Found { message: Some(format!("No lyrics from {}.", lyrics::source_name(source))), source: source.clone(), source_name: lyrics::source_name(source), ..Default::default() },
                    Err(_) => Found { message: Some(format!("Couldn't reach {}. Check your internet connection.", lyrics::source_name(source))), source: source.clone(), source_name: lyrics::source_name(source), ..Default::default() },
                },
                None => lyrics::find(&query, choice.as_deref()),
            };
            let _ = send.send(Event::Lyrics(id, found));
            ctx.request_repaint();
        });
    }

    // A source picked in the menu (None: "Best match") becomes this song's choice
    pub fn pick_lyrics_source(&mut self, source: Option<&str>) {
        let Some(id) = self.current.clone() else { return };
        self.library.change(&id, |record| record.lyrics = source.map(str::to_string));
        self.load_lyrics(&id, source);
    }

    // Timed lyrics a little ahead of the singing or behind it, like a video player's
    // subtitle delay: + shows them later, - earlier (until saved)
    pub fn move_lyrics(&mut self, seconds: f32) {
        let Some(view) = self.lyrics.as_mut().filter(|view| !view.lines.is_empty()) else { return };
        let before = view.offset;
        view.offset = ((before + seconds) * 100.0).round().clamp(-6000.0, 6000.0) / 100.0;
        view.scrolled_at = None; // follow the singing again right away
        if before == 0.0 && view.offset != 0.0 {
            self.show_toast("Lyrics moved. Save them (the button above the lyrics) to keep the new timing.".into());
        }
    }

    // Lyrics without timing get a first guess, then you tap along to fix it; timed ones
    // can be tapped again the same way (their times are the guess)
    pub fn start_tapping(&mut self) {
        let duration = if self.duration > 0.0 { self.duration } else { self.current_song().map_or(0.0, |s| s.duration) };
        let Some(view) = self.lyrics.as_mut() else { return };
        let Some(found) = view.found.as_mut() else { return };
        if view.lines.is_empty() {
            let Some(plain) = found.plain.clone() else { return };
            let texts: Vec<String> = plain.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
            if texts.is_empty() {
                return;
            }
            let guesses = lyrics::estimate(&texts, duration);
            view.lines = texts.into_iter().zip(&guesses).map(|(text, &time)| lyrics::Line { time, text }).collect();
        }
        for line in &mut view.lines {
            line.time = (line.time + view.offset).max(0.0); // (a timing move becomes part of the times)
        }
        view.offset = 0.0;
        let guesses: Vec<f32> = view.lines.iter().map(|l| l.time).collect();
        view.tapping = Some(Tapping { next: 0, tapped: vec![false; guesses.len()], guesses, history: Vec::new() });
        found.synced = Some(lyrics::to_lrc(&view.lines));
        found.saved = false;
        self.show_toast("Play the song and press Space (or Tap) as each highlighted line starts. A few taps are often enough.".into());
    }

    // The highlighted line starts now (or a line you clicked)
    pub fn tap(&mut self, line: Option<usize>) {
        let now = self.player.position();
        let Some(view) = self.lyrics.as_mut() else { return };
        let Some(tapping) = view.tapping.as_mut() else { return };
        let k = line.unwrap_or(tapping.next);
        if k >= view.lines.len() {
            return;
        }
        let mut times: Vec<f32> = view.lines.iter().map(|l| l.time).collect();
        tapping.history.push((tapping.next, times.clone(), tapping.tapped.clone()));
        tapping.tapped[k] = true;
        lyrics::retime(&mut times, &tapping.guesses, &tapping.tapped, k, now);
        for (line, time) in view.lines.iter_mut().zip(times) {
            line.time = time;
        }
        tapping.next = (k + 1).min(view.lines.len());
        view.scrolled_at = None;
        if let Some(found) = view.found.as_mut() {
            found.synced = Some(lyrics::to_lrc(&view.lines));
        }
    }

    pub fn undo_tap(&mut self) {
        let Some(view) = self.lyrics.as_mut() else { return };
        let Some(tapping) = view.tapping.as_mut() else { return };
        if let Some((next, times, tapped)) = tapping.history.pop() {
            for (line, time) in view.lines.iter_mut().zip(times) {
                line.time = time;
            }
            tapping.next = next;
            tapping.tapped = tapped;
            if let Some(found) = view.found.as_mut() {
                found.synced = Some(lyrics::to_lrc(&view.lines));
            }
        }
    }

    pub fn finish_tapping(&mut self) {
        if let Some(view) = self.lyrics.as_mut() {
            view.tapping = None;
        }
        self.show_toast("Timing done. Save the lyrics (the button above them) to keep it.".into());
    }

    pub fn reset_lyrics_timing(&mut self) {
        if let Some(view) = self.lyrics.as_mut() {
            view.offset = 0.0;
        }
    }

    // Saves the lyrics on screen in the lyrics folder, with the timing you set
    pub fn save_lyrics(&mut self) {
        let Some(song) = self.current_song() else { return };
        let (artist, title, album, id) = (if song.known_artist() { song.artist.clone() } else { String::new() }, song.title.clone(), song.album.clone(), song.id.clone());
        let duration = self.duration;
        let Some(view) = self.lyrics.as_mut() else { return };
        let Some(found) = view.found.as_mut() else { return };
        let offset = view.offset;
        let synced = found.synced.as_ref().map(|text| if offset != 0.0 { lyrics::shift_lrc(text, offset) } else { text.clone() });
        let text = match &synced {
            Some(synced) => lyrics::lrc_file_text(&artist, &title, &album, duration, synced),
            None => match &found.plain {
                Some(plain) => plain.clone(),
                None => return,
            },
        };
        match lyrics::save(&self.data.join("lyrics"), &lyrics::file_name(&artist, &title), &text, synced.is_some()) {
            Ok(file) => {
                // The moved times are the lyrics' own times now
                if offset != 0.0 {
                    for line in &mut view.lines {
                        line.time = (line.time + offset).max(0.0);
                    }
                    found.synced = synced;
                    view.offset = 0.0;
                }
                found.saved = true;
                found.source = "saved".into();
                found.source_name = lyrics::source_name("saved");
                // Retimed lyrics become this song's choice, so they win over a .lrc next to the song
                if offset != 0.0 {
                    self.library.change(&id, |record| record.lyrics = Some("saved".into()));
                }
                self.show_toast(format!("Saved{} in the lyrics folder as \"{file}\".", if offset != 0.0 { " with the new timing" } else { "" }));
            }
            Err(error) => self.show_toast(format!("Couldn't save the lyrics: {error}")),
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.prefs.volume = volume.clamp(0.0, 1.0);
        self.prefs.muted = false;
        self.player.set_volume(self.prefs.volume);
    }

    pub fn toggle_mute(&mut self) {
        self.prefs.muted = !self.prefs.muted;
        self.player.set_volume(if self.prefs.muted { 0.0 } else { self.prefs.volume });
    }

    // ---------- Covers ----------

    // Asks for a song's cover once (big: also the large picture, for the song playing)
    pub fn request_cover(&mut self, id: &str, big: bool) {
        if !self.requested.insert((id.to_string(), big)) {
            return;
        }
        let Some(song) = self.song(id) else { return };
        let album_known = !song.album.is_empty() && (!song.tags.album.is_empty() || song.album != names::guess_from_file_name(&song.rel).album);
        let request = covers::Request {
            id: id.to_string(),
            path: song.path.clone(),
            folder_image: self.folder_covers.get(&song.folder).map(|rel| self.folder.join(rel)),
            artist: if song.known_artist() { names::lookup_name(&song.artist) } else { String::new() },
            album: if album_known { song.album.clone() } else { String::new() },
            title: names::lookup_name(&song.title),
            big,
        };
        self.covers.request(request);
    }

    // ---------- Favorites ----------

    pub fn is_favorite(&self, id: &str) -> bool {
        self.library.get(id).is_some_and(|record| record.favorite.is_some())
    }

    pub fn toggle_favorite(&mut self, id: &str) {
        let on = !self.is_favorite(id);
        self.library.change(id, |record| record.favorite = on.then(store::now_iso));
        if self.prefs.only_favorites {
            self.view_dirty = true;
        }
    }

    // Hearted songs, the latest first: (id, title, artist). A favorite keeps its names in
    // its record, so the list looks complete even for songs in another folder.
    pub fn favorites(&self) -> Vec<(String, String, String)> {
        let mut list: Vec<(&String, &store::Record)> = self.library.songs.iter().filter(|(_, r)| r.favorite.is_some()).collect();
        list.sort_by(|a, b| b.1.favorite.cmp(&a.1.favorite));
        list.into_iter()
            .map(|(id, record)| match self.song(id) {
                Some(song) => (id.clone(), song.title.clone(), song.artist.clone()),
                None => {
                    let title = record.title.clone().unwrap_or_else(|| id.split('|').next().unwrap_or("").to_string());
                    let artist = record.artist.clone().filter(|a| !a.is_empty()).unwrap_or_else(|| UNKNOWN_ARTIST.into());
                    (id.clone(), title, artist)
                }
            })
            .collect()
    }

    pub fn play_favorite(&mut self, id: &str) {
        if self.song(id).is_none() {
            self.show_toast("That song isn't in the open folder.".into());
            return;
        }
        let list: Vec<String> = self.favorites().into_iter().map(|(id, _, _)| id).filter(|id| self.song(id).is_some()).collect();
        self.play_from_list(id, list);
    }

    pub fn plays_of(&self, id: &str) -> u64 {
        self.library.get(id).map_or(0, |record| record.plays)
    }

    // ---------- What "All songs" shows ----------

    pub fn set_sort(&mut self, sort: &str) {
        self.prefs.sort = sort.to_string();
        self.view_dirty = true;
    }

    pub fn set_only_favorites(&mut self, on: bool) {
        self.prefs.only_favorites = on;
        self.view_dirty = true;
    }

    // Shows only one artist (the same artist again shows everyone)
    pub fn filter_by_artist(&mut self, key: &str) {
        self.artist_filter = if self.artist_filter.as_deref() == Some(key) { None } else { Some(key.to_string()) };
        self.view_dirty = true;
    }

    pub fn filter_by_genre(&mut self, key: &str) {
        self.genre_filter = if self.genre_filter.as_deref() == Some(key) { None } else { Some(key.to_string()) };
        self.view_dirty = true;
    }

    pub fn clear_filters(&mut self, artist: bool, genre: bool) {
        if artist {
            self.artist_filter = None;
        }
        if genre {
            self.genre_filter = None;
        }
        self.view_dirty = true;
    }

    pub fn shown_ids(&self) -> Vec<String> {
        self.shown.iter().map(|&i| self.songs[i].id.clone()).collect()
    }

    fn rebuild_view(&mut self) {
        self.view_dirty = false;
        let artist_key = |song: &Song| if song.known_artist() { names::normalize(&song.artist) } else { String::new() };
        let first_genre = |song: &Song| names::genres_of(&song.genre).first().map_or(String::new(), |g| names::normalize(g));
        let mut shown: Vec<usize> = (0..self.songs.len())
            .filter(|&i| {
                let song = &self.songs[i];
                self.artist_filter.as_ref().is_none_or(|key| artist_key(song) == *key)
                    && (!self.prefs.only_favorites || self.is_favorite(&song.id))
                    && self.genre_filter.as_ref().is_none_or(|key| {
                        if key.is_empty() { song.genre.is_empty() } else { names::genres_of(&song.genre).iter().any(|g| names::normalize(g) == *key) }
                    })
            })
            .collect();

        let songs = &self.songs;
        let by_title = |a: &Song, b: &Song| natural_cmp(&a.title, &b.title).then_with(|| natural_cmp(&a.artist, &b.artist));
        let unknown_last = |a: &Song, b: &Song| (!a.known_artist()).cmp(&!b.known_artist());
        let plays: HashMap<&str, u64> = songs.iter().map(|s| (s.id.as_str(), self.plays_of(&s.id))).collect();
        match self.prefs.sort.as_str() {
            "title" => shown.sort_by(|&a, &b| by_title(&songs[a], &songs[b])),
            "artist" => shown.sort_by(|&a, &b| {
                let (a, b) = (&songs[a], &songs[b]);
                unknown_last(a, b).then_with(|| natural_cmp(&a.artist, &b.artist)).then_with(|| natural_cmp(&a.album, &b.album)).then_with(|| natural_cmp(&a.rel, &b.rel))
            }),
            "album" => shown.sort_by(|&a, &b| {
                let (a, b) = (&songs[a], &songs[b]);
                a.album.is_empty().cmp(&b.album.is_empty()).then_with(|| natural_cmp(&a.album, &b.album)).then_with(|| natural_cmp(&a.rel, &b.rel))
            }),
            "genre" => shown.sort_by(|&a, &b| {
                let (a, b) = (&songs[a], &songs[b]);
                a.genre.is_empty().cmp(&b.genre.is_empty())
                    .then_with(|| natural_cmp(&first_genre(a), &first_genre(b)))
                    .then_with(|| unknown_last(a, b))
                    .then_with(|| natural_cmp(&a.artist, &b.artist))
                    .then_with(|| by_title(a, b))
            }),
            "recent" => shown.sort_by(|&a, &b| songs[b].added.cmp(&songs[a].added).then_with(|| by_title(&songs[a], &songs[b]))),
            "plays" => shown.sort_by(|&a, &b| plays[songs[b].id.as_str()].cmp(&plays[songs[a].id.as_str()]).then_with(|| by_title(&songs[a], &songs[b]))),
            _ => {}
        }

        // Group names, for the sorts that have them
        let group_of = |song: &Song| -> Option<String> {
            match self.prefs.sort.as_str() {
                "title" => {
                    let letter = names::normalize(&song.title).chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                    Some(if letter.chars().next().is_some_and(char::is_alphabetic) { letter } else { "#".into() })
                }
                "artist" => Some(artist_key(song)),
                "album" => Some(names::normalize(&song.album)),
                "genre" => Some(self.genre_filter.clone().unwrap_or_else(|| first_genre(song))),
                _ => None,
            }
        };
        let mut rows = Vec::new();
        let mut last_group: Option<String> = None;
        let groups: Vec<Option<String>> = shown.iter().map(|&i| group_of(&songs[i])).collect();
        for (position, &i) in shown.iter().enumerate() {
            if let Some(group) = &groups[position] {
                if last_group.as_ref() != Some(group) {
                    let song = &songs[i];
                    let count = groups[position..].iter().take_while(|g| g.as_ref() == Some(group)).count();
                    let (name, sub) = match self.prefs.sort.as_str() {
                        "title" => (group.clone(), String::new()),
                        "artist" => (song.artist.clone(), String::new()),
                        "album" => (if song.album.is_empty() { "No album".into() } else { song.album.clone() }, song.artist.clone()),
                        _ => {
                            let name = if group.is_empty() { "No genre".to_string() } else {
                                names::genres_of(&song.genre).into_iter().find(|g| names::normalize(g) == *group).unwrap_or(group).to_string()
                            };
                            (name, String::new())
                        }
                    };
                    rows.push(Row::Group { key: group.clone(), name, sub, count });
                    last_group = Some(group.clone());
                }
            }
            rows.push(Row::Song(i));
        }
        self.shown = shown;
        self.rows = rows;
    }

    // ---------- Statistics ----------

    fn rebuild_stats(&mut self) {
        self.stats_dirty = false;
        let mut artists: HashMap<String, Bar> = HashMap::new();
        let mut genres: HashMap<String, Bar> = HashMap::new();
        let mut no_genre = 0;
        let mut albums = HashSet::new();
        for song in &self.songs {
            let key = if song.known_artist() { names::normalize(&song.artist) } else { String::new() };
            let name = if song.known_artist() { song.artist.clone() } else { UNKNOWN_ARTIST.to_string() };
            artists.entry(key.clone()).or_insert(Bar { key, name, songs: 0 }).songs += 1;
            let song_genres = names::genres_of(&song.genre);
            if song_genres.is_empty() {
                no_genre += 1;
            }
            let mut seen = HashSet::new();
            for genre in song_genres {
                let key = names::normalize(genre);
                if seen.insert(key.clone()) {
                    genres.entry(key.clone()).or_insert(Bar { key, name: genre.to_string(), songs: 0 }).songs += 1;
                }
            }
            if !song.album.is_empty() {
                albums.insert(format!("{}|{}", names::normalize(&song.album), names::normalize(&song.artist)));
            }
        }
        let sorted = |map: HashMap<String, Bar>| {
            let mut list: Vec<Bar> = map.into_values().collect();
            list.sort_by(|a, b| a.key.is_empty().cmp(&b.key.is_empty()).then(b.songs.cmp(&a.songs)).then_with(|| natural_cmp(&a.name, &b.name)));
            list
        };
        let mut genres = sorted(genres);
        if no_genre > 0 {
            genres.push(Bar { key: String::new(), name: "No genre".into(), songs: no_genre });
        }
        self.stats = Stats { songs: self.songs.len(), artists: sorted(artists), genres, albums: albums.len() };
        // Songs with the same artist and title are copies of each other
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        for song in self.songs.iter().filter(|s| s.known_artist()) {
            groups.entry(format!("{}|{}", names::simplify(&song.artist), names::simplify(&song.title))).or_default().push(song.id.clone());
        }
        groups.retain(|_, ids| ids.len() > 1);
        self.dup_groups = groups;
    }

    // ---------- Search ----------

    fn run_search(&mut self) {
        self.searched = self.search.clone();
        let words: Vec<String> = names::normalize(self.search.trim()).split_whitespace().map(str::to_string).collect();
        self.search_results = if words.is_empty() {
            Vec::new()
        } else {
            (0..self.songs.len()).filter(|&i| words.iter().all(|w| self.songs[i].search.contains(w.as_str()))).take(60).collect()
        };
    }

    // ---------- Messages ----------

    pub fn show_toast(&mut self, message: String) {
        self.toast = Some((message, Instant::now()));
    }

    pub fn set_theme(&mut self, id: &'static str) {
        self.prefs.theme = id.to_string();
        self.theme = theme::by_id(id);
    }

    pub fn import_old_library(&mut self) {
        let Some(folder) = rfd::FileDialog::new().set_title("Pick the old player's folder (where server.py is)").pick_folder() else {
            return;
        };
        match store::import_old_player(&folder, &self.data) {
            Ok(message) => {
                self.library = Library::load(&self.data);
                self.welcome = None;
                self.prefs.save(&self.data);
                self.show_toast(message);
                self.textures.clear();
                self.requested.clear();
                self.open_starting_folder();
            }
            Err(message) => self.welcome = Some(Welcome { message: Some(message) }),
        }
    }

    pub fn start_fresh(&mut self) {
        self.welcome = None;
        self.prefs.save(&self.data); // (there's a preferences.json now: no welcome next time)
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        debug("frame: start");
        self.handle_events();
        self.tick();
        debug("frame: drawing");
        crate::ui::draw(self, ctx);
        debug("frame: done");
        if self.eq_moving || self.old_backdrop.is_some() {
            ctx.request_repaint_after(Duration::from_millis(33)); // smooth bars and fades
        } else if self.player.is_playing() || self.tags_waiting > 0 {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_all();
    }
}

// ---------- Small helpers ----------

pub fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
}

// Like sorting by hand: ignores case and accents, and "Track 2" comes before "Track 10"
pub fn natural_cmp(a: &str, b: &str) -> Order {
    let (a, b) = (names::normalize(a), names::normalize(b));
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Order::Equal,
            (None, _) => return Order::Less,
            (_, None) => return Order::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        it.next();
                    }
                    n.trim_start_matches('0').to_string()
                };
                let (m, n) = (take(&mut x), take(&mut y));
                match m.len().cmp(&n.len()).then_with(|| m.cmp(&n)) {
                    Order::Equal => {}
                    other => return other,
                }
            }
            (Some(c), Some(d)) => {
                match c.cmp(&d) {
                    Order::Equal => {
                        x.next();
                        y.next();
                    }
                    other => return other,
                }
            }
        }
    }
}

pub fn format_time(seconds: f32) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{}:{:02}", total / 60, total % 60)
}
