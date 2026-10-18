// Music Player: your music folder with covers, favorites, statistics and themes.
// The modules, roughly from the bottom up:
//   names    how songs get their names (tidying "(Official Video)" and the like)
//   store    library.json, settings and preferences on disk
//   library  the songs in the folder, their tags, watching for changes
//   covers   cover pictures (from the file, its folder, or online)
//   player   sound (analysis: the equalizer; loudness: Even volume)
//   lyrics   lyrics from files and online, timing and saving
//   edit     changing files: tags, renaming, the Trash
//   wiki     albums and genres from Wikipedia
//   downloads  songs from YouTube and other sites (with yt-dlp)
//   transfer   the library as one .zip (export and import)
//   media    media keys and the desktop's media controls
//   app      what the app knows and does
//   ui       what's on screen
mod analysis;
mod app;
mod covers;
mod downloads;
mod edit;
mod genres;
mod library;
mod loudness;
mod lyrics;
mod media;
mod names;
mod player;
mod store;
mod theme;
mod transfer;
mod ui;
mod wiki;

use eframe::egui;

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Music Player")
        .with_app_id("music-player") // matches the .desktop file, so the desktop shows the right icon
        .with_inner_size([1380.0, 880.0])
        .with_min_inner_size([1240.0, 680.0]);
    if let Ok(icon) = image::load_from_memory(include_bytes!("../assets/icon.png")) {
        let icon = icon.to_rgba8();
        let (width, height) = icon.dimensions();
        viewport = viewport.with_icon(egui::IconData { rgba: icon.into_raw(), width, height });
    }
    app::debug("main: opening the window");
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Music Player", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
