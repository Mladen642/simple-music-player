// Downloads: songs (or whole playlists) from YouTube and many other sites, saved into
// the open folder as "Artist - Title.mp3" with their names in the tags.
// It's done by three free programs:
//   yt-dlp  finds and downloads the audio (and keeps itself up to date: sites change)
//   ffmpeg  turns it into mp3
//   deno    lets yt-dlp read YouTube's pages
// Ones already on the computer are used; otherwise "Get them" downloads them once
// into the player's data folder (bin/).
use crate::names::{self, Names};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

pub const PARALLEL: usize = 3; // songs at the same time (more makes YouTube slow you down)

#[derive(Clone, Default)]
pub struct Tools {
    pub yt_dlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub deno: Option<PathBuf>,
    pub bin: PathBuf, // the player's own copies live here
}

impl Tools {
    pub fn find(data: &Path) -> Tools {
        let bin = data.join("bin");
        let pick = |name: &str| {
            let own = bin.join(name);
            if own.is_file() { Some(own) } else { which(name) }
        };
        Tools { yt_dlp: pick("yt-dlp"), ffmpeg: pick("ffmpeg"), deno: pick("deno"), bin }
    }

    pub fn ready(&self) -> bool {
        self.yt_dlp.is_some() && self.ffmpeg.is_some()
    }

    // yt-dlp with the player's bin folder first on its PATH (so it finds deno and ffmpeg)
    fn command(&self) -> Command {
        let mut command = Command::new(self.yt_dlp.as_ref().expect("checked by ready()"));
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.bin.clone()];
        paths.extend(std::env::split_paths(&path));
        command.env("PATH", std::env::join_paths(paths).unwrap_or(path));
        command.stdin(Stdio::null());
        command
    }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(name)).find(|p| p.is_file())
}

// ---------- Getting the tools ----------

const YT_DLP: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";
const FFMPEG: &str = "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz";
const DENO: &str = "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-unknown-linux-gnu.zip";

fn fetch(url: &str, to: &Path) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_secs(20)).build();
    let response = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut file = std::fs::File::create(to).map_err(|e| e.to_string())?;
    std::io::copy(&mut response.into_reader(), &mut file).map_err(|e| e.to_string())?;
    Ok(())
}

fn executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

// Downloads whatever is missing (in the background: about 150 MB)
pub fn get_tools(tools: &Tools) -> Result<(), String> {
    std::fs::create_dir_all(&tools.bin).map_err(|e| e.to_string())?;
    let temp = tools.bin.join("downloading");
    if tools.yt_dlp.is_none() {
        fetch(YT_DLP, &temp)?;
        executable(&temp)?;
        std::fs::rename(&temp, tools.bin.join("yt-dlp")).map_err(|e| e.to_string())?;
    }
    if tools.ffmpeg.is_none() {
        let archive = tools.bin.join("ffmpeg.tar.xz");
        fetch(FFMPEG, &archive)?;
        // tar knows .xz on every Linux system; only ffmpeg itself is kept
        let unpacked = tools.bin.join("ffmpeg-unpacked");
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        let status = Command::new("tar").arg("-xJf").arg(&archive).arg("-C").arg(&unpacked).arg("--wildcards").arg("*/bin/ffmpeg").status().map_err(|e| e.to_string())?;
        let found = find_file(&unpacked, "ffmpeg");
        match (status.success(), found) {
            (true, Some(ffmpeg)) => std::fs::rename(ffmpeg, tools.bin.join("ffmpeg")).map_err(|e| e.to_string())?,
            _ => return Err("Couldn't unpack ffmpeg.".into()),
        }
        let _ = std::fs::remove_dir_all(&unpacked);
        let _ = std::fs::remove_file(&archive);
        executable(&tools.bin.join("ffmpeg"))?;
    }
    if tools.deno.is_none() {
        let archive = tools.bin.join("deno.zip");
        fetch(DENO, &archive)?;
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let mut entry = zip.by_name("deno").map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        std::fs::write(tools.bin.join("deno"), bytes).map_err(|e| e.to_string())?;
        executable(&tools.bin.join("deno"))?;
        let _ = std::fs::remove_file(&archive);
    }
    Ok(())
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|f| f == name) {
            return Some(path);
        }
    }
    None
}

// The player's own yt-dlp updates itself once a day (a system one is the system's job)
pub fn update_yt_dlp(tools: &Tools) {
    let Some(yt_dlp) = &tools.yt_dlp else { return };
    if !yt_dlp.starts_with(&tools.bin) {
        return;
    }
    let stamp = tools.bin.join("yt-dlp.checked");
    let recent = std::fs::metadata(&stamp).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 24 * 3600);
    if !recent {
        let _ = tools.command().arg("-U").stdout(Stdio::null()).stderr(Stdio::null()).status();
        let _ = std::fs::write(stamp, b"");
    }
}

// ---------- A link: one song, or a playlist ----------

#[derive(Clone)]
pub struct Entry {
    pub url: String,
    pub artist: String,
    pub title: String,
    pub chosen: bool,
}

// What a link holds (without downloading): the playlist's name, if it's one, and its songs
pub fn check(tools: &Tools, link: &str) -> Result<(String, Vec<Entry>), String> {
    let output = tools.command().args(["--flat-playlist", "-J", "--no-warnings", link]).output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(message.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("yt-dlp couldn't read that link.").replace("ERROR: ", ""));
    }
    let info: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let entry = |item: &Value, fallback_url: &str| -> Entry {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        // Music sites name the artist and track; video sites only the uploader and title
        let tags = Names {
            title: if text("track").is_empty() { text("title") } else { text("track") },
            artist: [text("artist"), text("creator"), text("uploader"), text("channel")].into_iter().find(|t| !t.is_empty()).unwrap_or_default(),
            ..Default::default()
        };
        let named = names::natural_names(&tags, "");
        let url = [text("webpage_url"), text("url")].into_iter().find(|t| t.starts_with("http")).unwrap_or_else(|| fallback_url.to_string());
        Entry { url, artist: if named.artist == names::UNKNOWN_ARTIST { String::new() } else { named.artist }, title: named.title, chosen: true }
    };
    match info.get("entries").and_then(Value::as_array) {
        Some(list) => {
            let name = info.get("title").and_then(Value::as_str).unwrap_or("Playlist").to_string();
            Ok((name, list.iter().filter(|item| !item.is_null()).map(|item| entry(item, "")).filter(|e| !e.url.is_empty()).collect()))
        }
        None => Ok((String::new(), vec![entry(&info, link)])),
    }
}

// ---------- Downloading ----------

#[derive(Clone, PartialEq, Debug)]
pub enum State {
    Waiting,
    Downloading(f32), // percent
    Converting,
    Done(String),     // the file's name
    Failed(String),
    Cancelled,
}

// Running yt-dlp processes, so Cancel can stop them
pub type Running = Arc<Mutex<Vec<(usize, Child)>>>;

// Downloads one song into the folder as "Artist - Title.mp3" (never over another file),
// then writes its names into the tags. progress: called with the new state.
pub fn download(tools: &Tools, folder: &Path, job: usize, entry: &Entry, running: &Running, progress: &dyn Fn(State)) -> State {
    let base = crate::edit::file_name_for("x.mp3", &entry.title, if entry.artist.is_empty() { names::UNKNOWN_ARTIST } else { &entry.artist });
    let base = base.strip_suffix(".mp3").unwrap_or("song").to_string();
    let mut name = format!("{base}.mp3");
    let mut n = 2;
    while folder.join(&name).exists() || folder.join(format!("{}.part", name)).exists() {
        name = format!("{base} ({n}).mp3");
        n += 1;
    }
    let stem = name.strip_suffix(".mp3").unwrap_or(&name).to_string();
    let template = folder.join(format!("{}.%(ext)s", stem.replace('%', "%%")));
    let mut command = tools.command();
    command.args(["-f", "bestaudio/best", "-x", "--audio-format", "mp3", "--audio-quality", "0", "--no-playlist", "--no-mtime", "--newline"])
        .args(["--progress-template", "download:%(progress._percent_str)s"])
        .arg("--ffmpeg-location").arg(tools.ffmpeg.as_ref().map(|f| f.parent().unwrap_or(f)).unwrap_or(&tools.bin))
        .arg("-o").arg(&template).arg(&entry.url)
        .stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return State::Failed(error.to_string()),
    };
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    running.lock().unwrap().push((job, child));
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });
    if let Some(stdout) = stdout {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let line = line.trim();
            if let Some(percent) = line.strip_suffix('%').and_then(|p| p.trim().parse::<f32>().ok()) {
                progress(State::Downloading(percent));
            } else if line.contains("[ExtractAudio]") {
                progress(State::Converting);
            }
        }
    }
    let child = { let mut list = running.lock().unwrap(); list.iter().position(|(j, _)| *j == job).map(|i| list.remove(i).1) };
    let Some(mut child) = child else {
        // Cancelled: its pieces go away
        cleanup(folder, &stem);
        return State::Cancelled;
    };
    let ok = child.wait().is_ok_and(|status| status.success());
    let errors = errors.join().unwrap_or_default();
    let file = folder.join(&name);
    if !ok || !file.exists() {
        cleanup(folder, &stem);
        let reason = errors.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("the download failed").replace("ERROR: ", "");
        return State::Failed(reason.chars().take(160).collect());
    }
    let names = Names { title: entry.title.clone(), artist: entry.artist.clone(), ..Default::default() };
    let _ = crate::edit::write_tags(&file, &names);
    State::Done(name)
}

// Half-downloaded pieces of a song that didn't finish
fn cleanup(folder: &Path, stem: &str) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&format!("{stem}.")) && !name.ends_with(".mp3") || name == format!("{stem}.mp3.part") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub fn cancel(running: &Running) {
    for (_, child) in running.lock().unwrap().iter_mut() {
        let _ = child.kill();
    }
    running.lock().unwrap().clear();
}

#[cfg(test)]
mod tests {
    // A song served by a local web server: checked, downloaded, made into an mp3, named
    // and tagged. Needs yt-dlp and ffmpeg on the PATH, python3, and an audio file:
    //   MUSIC_PLAYER_TEST_AUDIO=some.m4a cargo test -- --ignored
    #[test]
    #[ignore]
    fn a_download() {
        let audio = std::path::PathBuf::from(std::env::var("MUSIC_PLAYER_TEST_AUDIO").expect("MUSIC_PLAYER_TEST_AUDIO"));
        let tools = super::Tools::find(&std::env::temp_dir().join("no-own-tools"));
        assert!(tools.ready(), "yt-dlp and ffmpeg are needed on the PATH");
        let dir = audio.parent().unwrap().to_string_lossy().to_string();
        let mut server = std::process::Command::new("python3").args(["-m", "http.server", "8899", "-d", &dir])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
        let link = format!("http://127.0.0.1:8899/{}", audio.file_name().unwrap().to_string_lossy());
        let (_, mut entries) = super::check(&tools, &link).unwrap();
        entries[0].artist = "Test Artist".into();
        entries[0].title = "Test Song (Official Video)".into();
        let out = std::env::temp_dir().join("download-test");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let state = super::download(&tools, &out, 0, &entries[0], &super::Running::default(), &|_| {});
        server.kill().unwrap();
        assert_eq!(state, super::State::Done("Test Artist - Test Song.mp3".into()));
        let (tags, _, _) = crate::library::read_tags(&out.join("Test Artist - Test Song.mp3"));
        assert_eq!((tags.artist.as_str(), tags.title.as_str()), ("Test Artist", "Test Song (Official Video)"));
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1, "nothing left over");
    }
}
