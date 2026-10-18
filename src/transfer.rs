// Moving your library to another computer (or keeping a copy): one .zip with everything
// the player knows about your songs: names, albums, genres, hearts, plays, lyrics
// choices, covers, saved lyrics, settings. Not the music files themselves.
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const FILES: [&str; 7] = ["library.json", "settings.json", "preferences.json", "loudness.json", "wikipedia.json", "genres.json", "last-rename.json"];
const FOLDERS: [&str; 2] = ["covers", "lyrics"];

// Writes the .zip; says how many files went in
pub fn export(data: &Path, to: &Path) -> Result<usize, String> {
    let file = File::create(to).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut count = 0;
    let mut add = |zip: &mut zip::ZipWriter<File>, path: &Path, name: &str| -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
        count += 1;
        Ok(())
    };
    for name in FILES {
        let path = data.join(name);
        if path.is_file() {
            add(&mut zip, &path, name)?;
        }
    }
    for folder in FOLDERS {
        let Ok(entries) = std::fs::read_dir(data.join(folder)) else { continue };
        for entry in entries.flatten().filter(|e| e.path().is_file()) {
            let name = format!("{folder}/{}", entry.file_name().to_string_lossy());
            add(&mut zip, &entry.path(), &name)?;
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(count)
}

// Brings a .zip made by Export (or a zipped data folder) in. What's there now is kept
// first as a backup in the "backups" folder. Says how many songs came along.
pub fn import(data: &Path, from: &Path) -> Result<String, String> {
    let file = File::open(from).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "That isn't a .zip file the player can read.".to_string())?;
    // The library may be at the top, or inside one folder (a zipped data folder)
    let mut names: Vec<PathBuf> = (0..zip.len()).filter_map(|i| zip.by_index(i).ok()?.enclosed_name()).collect();
    names.sort_by_key(|n| n.components().count());
    let library = names.iter().find(|n| n.file_name().is_some_and(|f| f == "library.json")).ok_or("That .zip has no library.json in it.")?;
    let prefix = library.parent().map(Path::to_path_buf).unwrap_or_default();

    if data.join("library.json").exists() {
        let backups = data.join("backups");
        std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
        export(data, &backups.join(format!("before-import-{}.zip", crate::store::now_iso().replace(':', "-"))))?;
    }
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name() else { continue }; // (never outside the data folder)
        let Ok(inside) = name.strip_prefix(&prefix) else { continue };
        let parts: Vec<String> = inside.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        let wanted = match parts.as_slice() {
            [file] => FILES.contains(&file.as_str()),
            [folder, _] => FOLDERS.contains(&folder.as_str()),
            _ => false,
        };
        if !wanted || entry.is_dir() {
            continue;
        }
        let target = data.join(inside);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
    }
    let songs = crate::store::Library::load(data).songs.len();
    Ok(format!("Imported: {songs} songs, with their names, hearts, plays, covers and lyrics."))
}

#[cfg(test)]
mod tests {
    #[test]
    fn there_and_back() {
        let old = crate::store::tests::sample_library("export-test-old");
        let (a, b) = (std::env::temp_dir().join("export-a"), std::env::temp_dir().join("export-b"));
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        crate::store::import_old_player(&old, &a).unwrap();
        let zip = std::env::temp_dir().join("library-test.zip");
        assert_eq!(super::export(&a, &zip).unwrap(), 4); // library, settings, a cover, lyrics
        std::fs::write(b.join("library.json"), "{\"songs\": {}}").unwrap(); // something to back up
        let message = super::import(&b, &zip).unwrap();
        assert!(message.contains("2 songs"), "{message}");
        assert_eq!(std::fs::read(a.join("library.json")).unwrap(), std::fs::read(b.join("library.json")).unwrap());
        assert!(b.join("covers/abc.jpg").exists() && b.join("lyrics/Band - Song.lrc").exists());
        assert_eq!(std::fs::read_dir(b.join("backups")).unwrap().count(), 1);
    }
}
