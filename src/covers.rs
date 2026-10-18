// Cover pictures. A song's cover comes from, in this order:
//   1. the picture stored inside its file,
//   2. a picture in its folder (cover.jpg, folder.jpg...),
//   3. online: iTunes, then Deezer, then MusicBrainz. Found covers are kept in the
//      "covers" folder (named like the old player named them, so those are used too),
//      and "no cover anywhere" is remembered for 30 days.
// Two background workers do this: one reads files (fast), one asks online (slow, one
// lookup at a time, as the sources ask). The song that's playing always goes first.
use crate::names::{same_name, simplify};
use eframe::egui::ColorImage;
use lofty::config::ParseOptions;
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::probe::Probe;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const BIG: u32 = 640;  // now playing
pub const SMALL: u32 = 96; // lists and the player bar

#[derive(Clone)]
pub struct Request {
    pub id: String,
    pub path: PathBuf,
    pub folder_image: Option<PathBuf>,
    pub artist: String, // empty when unknown: then nothing is looked up online
    pub album: String,  // empty unless the album is really known
    pub title: String,
    pub big: bool,      // also needed large (the song that's playing)
}

pub struct Cover {
    pub id: String,
    pub big: Option<ColorImage>,
    pub backdrop: Option<ColorImage>, // tiny and blurred: the window's background while it plays
    pub small: Option<ColorImage>, // None for both: this song has no cover anywhere
}

// A waiting line for a worker, where urgent jobs can cut in
struct Line<T> {
    jobs: Mutex<VecDeque<T>>,
    ready: Condvar,
}

impl<T> Line<T> {
    fn new() -> Arc<Line<T>> {
        Arc::new(Line { jobs: Mutex::new(VecDeque::new()), ready: Condvar::new() })
    }
    fn push(&self, job: T, urgent: bool) {
        let mut jobs = self.jobs.lock().unwrap();
        if urgent { jobs.push_front(job) } else { jobs.push_back(job) }
        self.ready.notify_one();
    }
    fn take(&self) -> T {
        let mut jobs = self.jobs.lock().unwrap();
        loop {
            if let Some(job) = jobs.pop_front() {
                return job;
            }
            jobs = self.ready.wait(jobs).unwrap();
        }
    }
}

pub struct Covers {
    files: Arc<Line<Request>>,
}

impl Covers {
    pub fn start(covers_dir: PathBuf, send: Sender<crate::app::Event>, wake: impl Fn() + Send + Sync + 'static) -> Covers {
        let files: Arc<Line<Request>> = Line::new();
        let online: Arc<Line<Request>> = Line::new();
        let wake = Arc::new(wake);

        // 1 and 2: the song's own picture, or its folder's
        let (line, next, send_files, wake_files) = (files.clone(), online.clone(), send.clone(), wake.clone());
        std::thread::spawn(move || loop {
            let request = line.take();
            let bytes = embedded_picture(&request.path).or_else(|| request.folder_image.as_ref().and_then(|p| std::fs::read(p).ok()));
            match bytes.as_deref().and_then(|bytes| decode(bytes, request.big)) {
                Some((big, small)) => {
                    let backdrop = big.as_ref().map(|_| backdrop(bytes.as_deref().unwrap_or_default()));
                    let _ = send_files.send(crate::app::Event::Cover(Cover { id: request.id, big, backdrop: backdrop.flatten(), small: Some(small) }));
                    wake_files();
                }
                None if !request.artist.is_empty() => next.push(request.clone(), request.big),
                None => {
                    let _ = send_files.send(crate::app::Event::Cover(Cover { id: request.id, big: None, backdrop: None, small: None }));
                    wake_files();
                }
            }
        });

        // 3: online, one lookup at a time
        std::thread::spawn(move || {
            let mut lookup = Lookup::default();
            loop {
                let request = online.take();
                let bytes = lookup.cover(&covers_dir, &request);
                let decoded = bytes.as_deref().and_then(|bytes| decode(bytes, request.big));
                let backdrop = if request.big { bytes.as_deref().and_then(backdrop) } else { None };
                let (big, small) = decoded.map_or((None, None), |(big, small)| (big, Some(small)));
                let _ = send.send(crate::app::Event::Cover(Cover { id: request.id, big, backdrop, small }));
                wake();
            }
        });
        Covers { files }
    }

    pub fn request(&self, request: Request) {
        let urgent = request.big;
        self.files.push(request, urgent);
    }
}

fn embedded_picture(path: &Path) -> Option<Vec<u8>> {
    let options = ParseOptions::new().read_properties(false);
    let file = Probe::open(path).ok()?.options(options).guess_file_type().ok()?.read().ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let pictures = tag.pictures();
    let picture = pictures.iter().find(|p| p.pic_type() == PictureType::CoverFront).or_else(|| pictures.first())?;
    Some(picture.data().to_vec())
}

// The picture at two sizes (the big one only when asked for), cut to a square
fn decode(bytes: &[u8], want_big: bool) -> Option<(Option<ColorImage>, ColorImage)> {
    let image = image::load_from_memory(bytes).ok()?;
    let side = image.width().min(image.height());
    let square = image.crop_imm((image.width() - side) / 2, (image.height() - side) / 2, side, side);
    let to_color = |img: image::DynamicImage| {
        let rgba = img.to_rgba8();
        ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
    };
    let big = want_big.then(|| to_color(if side > BIG { square.resize_exact(BIG, BIG, image::imageops::FilterType::Lanczos3) } else { square.clone() }));
    let small = to_color(square.resize_exact(SMALL, SMALL, image::imageops::FilterType::Triangle));
    Some((big, small))
}

// The cover shrunk to 48 pixels and blurred: stretched over the window, it becomes a
// soft wash of its colors (egui can't blur on screen, so it's done once, here)
fn backdrop(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?;
    let small = image.resize_exact(48, 48, image::imageops::FilterType::Triangle).blur(5.0).to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([48, 48], small.as_raw()))
}

// ---------- Online ----------

#[derive(Default)]
struct Lookup {
    last_itunes: Option<Instant>,
    last_musicbrainz: Option<Instant>,
}

const USER_AGENT: &str = "SimpleMusicPlayer/2.0 (personal music player)";

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(8)).user_agent(USER_AGENT).build()
}

fn get_json(url: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let mut request = agent().get(url);
    for (key, value) in query {
        request = request.query(key, value);
    }
    request.call().map_err(|e| e.to_string())?.into_json::<Value>().map_err(|e| e.to_string())
}

fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let response = agent().get(url).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

// The first item by the same artist, preferring one whose album (or title) matches too
fn best_match<'a>(items: &'a [Value], artist: &str, name: &str, artist_of: impl Fn(&Value) -> String, name_of: impl Fn(&Value) -> String) -> Option<&'a Value> {
    let by_artist: Vec<&Value> = items.iter().filter(|item| same_name(&artist_of(item), artist)).collect();
    by_artist.iter().find(|item| same_name(&name_of(item), name)).or(by_artist.first()).copied()
}

impl Lookup {
    // The cover from the covers folder, or found online (and then kept there)
    fn cover(&mut self, dir: &Path, request: &Request) -> Option<Vec<u8>> {
        let name_part = if request.album.is_empty() { &request.title } else { &request.album };
        if simplify(&request.artist).is_empty() || simplify(name_part).is_empty() {
            return None;
        }
        let key = format!("{}|{}", simplify(&request.artist), simplify(name_part));
        let name = &sha1_smol::Sha1::from(key.as_bytes()).digest().to_string()[..16];
        let image = dir.join(format!("{name}.jpg"));
        let missing = dir.join(format!("{name}.missing"));
        if let Ok(bytes) = std::fs::read(&image) {
            return Some(bytes);
        }
        let checked_recently = std::fs::metadata(&missing)
            .and_then(|m| m.modified())
            .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < Duration::from_secs(30 * 24 * 3600));
        if checked_recently {
            return None;
        }
        match self.find(&request.artist, &request.album, &request.title) {
            Ok(Some(bytes)) => {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(&image, &bytes);
                Some(bytes)
            }
            Ok(None) => {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(&missing, b"");
                None
            }
            Err(error) => {
                eprintln!("Couldn't look up a cover for {} / {name_part}: {error}", request.artist);
                None // offline or busy: tried again next time
            }
        }
    }

    // Asks the sources one after another: first for the album, then for the song
    // (album names from video sites can be anything). Err when a source couldn't be reached.
    fn find(&mut self, artist: &str, album: &str, title: &str) -> Result<Option<Vec<u8>>, String> {
        let searches: Vec<(&str, &str)> = if album.is_empty() { vec![("", title)] } else { vec![(album, ""), ("", title)] };
        let mut failed = Vec::new();
        for source in 0..3 {
            for (search_album, search_title) in &searches {
                let found = match source {
                    0 => self.itunes(artist, search_album, search_title),
                    1 => deezer(artist, search_album, search_title),
                    _ => self.musicbrainz(artist, search_album, search_title),
                };
                match found {
                    Ok(Some(bytes)) => return Ok(Some(bytes)),
                    Ok(None) => {}
                    Err(error) => {
                        failed.push(error);
                        break; // this source is down: on to the next one
                    }
                }
            }
        }
        if failed.is_empty() { Ok(None) } else { Err(failed.join("; ")) }
    }

    fn wait(last: &mut Option<Instant>, gap: Duration) {
        if let Some(last) = last {
            let since = last.elapsed();
            if since < gap {
                std::thread::sleep(gap - since);
            }
        }
        *last = Some(Instant::now());
    }

    // Apple's free iTunes catalog; covers at 600x600 (about 20 lookups a minute allowed)
    fn itunes(&mut self, artist: &str, album: &str, title: &str) -> Result<Option<Vec<u8>>, String> {
        let (entity, name, field) = if album.is_empty() { ("song", title, "trackName") } else { ("album", album, "collectionName") };
        Self::wait(&mut self.last_itunes, Duration::from_secs(3));
        let term = format!("{artist} {name}");
        let answer = get_json("https://itunes.apple.com/search", &[("term", &term), ("media", "music"), ("entity", entity), ("limit", "10")])?;
        let results: Vec<Value> = answer.get("results").and_then(Value::as_array).cloned().unwrap_or_default()
            .into_iter().filter(|item| !text(item, "artworkUrl100").is_empty()).collect();
        match best_match(&results, artist, name, |i| text(i, "artistName").into(), |i| text(i, field).into()) {
            Some(best) => get_bytes(&text(best, "artworkUrl100").replace("100x100bb", "600x600bb")).map(Some),
            None => Ok(None),
        }
    }

    // MusicBrainz, the open music encyclopedia, finds the album; the Cover Art Archive has
    // its cover (one request a second allowed)
    fn musicbrainz(&mut self, artist: &str, album: &str, title: &str) -> Result<Option<Vec<u8>>, String> {
        let clean = |text: &str| text.replace(['"', '\\'], "");
        Self::wait(&mut self.last_musicbrainz, Duration::from_millis(1100));
        let credited = |item: &Value| {
            item.get("artist-credit").and_then(Value::as_array).map_or(String::new(), |credits| {
                credits.iter().map(|c| text(c, "name")).collect::<Vec<_>>().join(" ")
            })
        };
        let releases: Vec<Value> = if album.is_empty() {
            let query = format!("recording:\"{}\" AND artist:\"{}\"", clean(title), clean(artist));
            let data = get_json("https://musicbrainz.org/ws/2/recording/", &[("query", &query), ("fmt", "json"), ("limit", "5")])?;
            data.get("recordings").and_then(Value::as_array).cloned().unwrap_or_default().iter()
                .filter(|recording| same_name(&credited(recording), artist))
                .flat_map(|recording| recording.get("releases").and_then(Value::as_array).cloned().unwrap_or_default())
                .collect()
        } else {
            let query = format!("release:\"{}\" AND artist:\"{}\"", clean(album), clean(artist));
            let data = get_json("https://musicbrainz.org/ws/2/release/", &[("query", &query), ("fmt", "json"), ("limit", "5")])?;
            data.get("releases").and_then(Value::as_array).cloned().unwrap_or_default().into_iter()
                .filter(|release| same_name(&credited(release), artist))
                .collect()
        };
        for release in releases.iter().take(5) {
            let id = text(release, "id");
            if id.is_empty() {
                continue;
            }
            if let Ok(bytes) = get_bytes(&format!("https://coverartarchive.org/release/{id}/front-500")) {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }
}

// Deezer's free catalog (no account needed); covers at 1000x1000
fn deezer(artist: &str, album: &str, title: &str) -> Result<Option<Vec<u8>>, String> {
    let (url, query) = if album.is_empty() {
        ("https://api.deezer.com/search", format!("artist:\"{artist}\" track:\"{title}\""))
    } else {
        ("https://api.deezer.com/search/album", format!("artist:\"{artist}\" album:\"{album}\""))
    };
    let answer = get_json(url, &[("q", &query), ("limit", "10")])?;
    if let Some(error) = answer.get("error") {
        return Err(format!("Deezer says: {error}"));
    }
    let cover_of = |item: &Value| -> String {
        let holder = if album.is_empty() { item.get("album").unwrap_or(&Value::Null) } else { item };
        let xl = text(holder, "cover_xl");
        if xl.is_empty() { text(holder, "cover_big").to_string() } else { xl.to_string() }
    };
    let results: Vec<Value> = answer.get("data").and_then(Value::as_array).cloned().unwrap_or_default()
        .into_iter().filter(|item| !cover_of(item).is_empty()).collect();
    let name = if album.is_empty() { title } else { album };
    match best_match(&results, artist, name, |i| text(i.get("artist").unwrap_or(&Value::Null), "name").into(), |i| text(i, "title").into()) {
        Some(best) => get_bytes(&cover_of(best)).map(Some),
        None => Ok(None),
    }
}
