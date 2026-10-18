// Changing songs on disk: names into their tags, tidy file names (and undoing the last
// rename), and deleting (to the Trash, never for good). Nothing here happens without
// you asking for it in the player.
use crate::names::{self, Names, UNKNOWN_ARTIST};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::Tag;
use serde_json::{Value, json};
use std::path::Path;

// Writes title, artist, album and genre into the file, so other players and your phone
// show them too. An empty name removes that tag. (Not every kind of file has tags:
// .webm and .aac files keep their names in library.json only.)
pub fn write_tags(path: &Path, names: &Names) -> Result<(), String> {
    let mut file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(file.primary_tag_type()));
    }
    let tag = file.primary_tag_mut().ok_or("this kind of file has no tags")?;
    let artist = if names.artist == UNKNOWN_ARTIST { "" } else { names.artist.trim() };
    let (title, album, genre) = (names.title.trim(), names.album.trim(), names.genre.trim());
    if title.is_empty() { tag.remove_title() } else { tag.set_title(title.to_string()) }
    if artist.is_empty() { tag.remove_artist() } else { tag.set_artist(artist.to_string()) }
    if album.is_empty() { tag.remove_album() } else { tag.set_album(album.to_string()) }
    if genre.is_empty() { tag.remove_genre() } else { tag.set_genre(genre.to_string()) }
    file.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

// Renames a file inside the music folder, never over another file. A change of
// upper/lower case only ("song.mp3" -> "Song.mp3") is fine.
pub fn rename(folder: &Path, from: &str, to: &str) -> Result<(), String> {
    let (old, new) = (folder.join(from), folder.join(to));
    if new.exists() && from.to_lowercase() != to.to_lowercase() {
        return Err(format!("\"{to}\" is there already"));
    }
    std::fs::rename(&old, &new).map_err(|e| e.to_string())?;
    // Lyrics next to the song go along
    let (old_lrc, new_lrc) = (old.with_extension("lrc"), new.with_extension("lrc"));
    if old_lrc.exists() && !new_lrc.exists() {
        let _ = std::fs::rename(old_lrc, new_lrc);
    }
    Ok(())
}

// ---------- The last rename, so it can be undone (last-rename.json, as before) ----------

pub fn save_undo(data: &Path, folder: &Path, renames: &[(String, String)]) {
    let renames: Vec<Value> = renames.iter().map(|(from, to)| json!({ "from": from, "to": to })).collect();
    let text = serde_json::to_string_pretty(&json!({ "folder": folder.to_string_lossy(), "renames": renames })).unwrap_or_default();
    let _ = crate::store::write_safely(&data.join("last-rename.json"), &text);
}

// The renames that can be undone in this folder (from, to), newest first
pub fn load_undo(data: &Path, folder: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(data.join("last-rename.json")) else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    if value.get("folder").and_then(Value::as_str) != Some(&*folder.to_string_lossy()) {
        return Vec::new();
    }
    let list = value.get("renames").and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter()
        .filter_map(|item| Some((item.get("from")?.as_str()?.to_string(), item.get("to")?.as_str()?.to_string())))
        .filter(|(_, to)| folder.join(to).exists())
        .collect()
}

pub fn clear_undo(data: &Path) {
    let _ = std::fs::remove_file(data.join("last-rename.json"));
}

// ---------- Deleting ----------

// To the Trash (and a .lrc next to the song with it)
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())?;
    let lrc = path.with_extension("lrc");
    if lrc.exists() {
        let _ = trash::delete(lrc);
    }
    Ok(())
}

// ---------- File names ----------

fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    }
}

// Every file is named "Artist - Title.ext", from the names the player shows (track
// numbers and extras like "(Official Video)" go). Without a known artist it's just the
// title, tidied. Empty when the name stays the same.
pub fn file_name_for(old_name: &str, title: &str, artist: &str) -> String {
    let (_, extension) = split_extension(old_name);
    let title = names::clean_name(title);
    let base = if artist == UNKNOWN_ARTIST || artist.trim().is_empty() { title } else { format!("{} - {title}", artist.trim()) };
    let base = crate::lyrics::safe_file_name(&base);
    let new_name = format!("{base}{}", extension.to_lowercase());
    if base.is_empty() || new_name == old_name { String::new() } else { new_name }
}

// ---------- Duplicates ----------

// Which copy of a song to keep: lossless first (FLAC, WAV), then the highest bitrate,
// then the biggest file
pub fn quality(extension: &str, bitrate: u32, size: u64) -> (bool, u32, u64) {
    (matches!(extension, "flac" | "wav"), bitrate, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(file_name_for("Carry On [El-qHgSWt3E].mp3", "Carry On", "Avenged Sevenfold"), "Avenged Sevenfold - Carry On.mp3");
        assert_eq!(file_name_for("03 - Intro.MP3", "Intro (Live)", "Band"), "Band - Intro (Live).mp3");
        assert_eq!(file_name_for("Band - Song.mp3", "Song", "Band"), "");
        assert_eq!(file_name_for("AC_DC - Song.mp3", "Title: Part 2", "AC/DC"), "AC-DC - Title - Part 2.mp3");
        assert_eq!(file_name_for("song.mp3", "Song (Official Video)", UNKNOWN_ARTIST), "Song.mp3");
    }

    // Needs an mp3 (it's copied, never changed):
    //   MUSIC_PLAYER_TEST_MP3=some.mp3 cargo test -- --ignored
    #[test]
    #[ignore]
    fn tags_go_into_the_file() {
        let source = std::fs::read(std::env::var("MUSIC_PLAYER_TEST_MP3").expect("MUSIC_PLAYER_TEST_MP3")).unwrap();
        let path = std::env::temp_dir().join("tag-test.mp3");
        std::fs::write(&path, source).unwrap();
        let wanted = Names { title: "Title".into(), artist: "Artist".into(), album: "".into(), genre: "Rock, Pop".into() };
        write_tags(&path, &wanted).unwrap();
        let (read, _, _) = crate::library::read_tags(&path);
        assert_eq!((read.title.as_str(), read.artist.as_str(), read.album.as_str(), read.genre.as_str()), ("Title", "Artist", "", "Rock, Pop"));
    }
}
