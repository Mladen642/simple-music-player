// Media keys and the desktop's media controls (the play/pause in GNOME's or KDE's panel,
// headphone buttons...), through MPRIS on Linux.
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
use std::sync::mpsc::Sender;
use std::time::Duration;

pub enum Command {
    Toggle,
    Play,
    Pause,
    Next,
    Previous,
    SeekTo(f32),
    SeekBy(f32),
    Raise,
}

pub struct Media {
    controls: Option<MediaControls>,
}

impl Media {
    pub fn start(send: Sender<crate::app::Event>, wake: impl Fn() + Send + 'static) -> Media {
        // Without the desktop's message bus (D-Bus) there are no media controls to talk to,
        // and the library would wait for it forever
        if !session_bus_exists() {
            return Media { controls: None };
        }
        let config = PlatformConfig { display_name: "Music Player", dbus_name: "music_player", hwnd: None };
        let controls = MediaControls::new(config).ok().and_then(|mut controls| {
            let attached = controls.attach(move |event| {
                let command = match event {
                    MediaControlEvent::Toggle => Command::Toggle,
                    MediaControlEvent::Play => Command::Play,
                    MediaControlEvent::Pause | MediaControlEvent::Stop => Command::Pause,
                    MediaControlEvent::Next => Command::Next,
                    MediaControlEvent::Previous => Command::Previous,
                    MediaControlEvent::SetPosition(MediaPosition(to)) => Command::SeekTo(to.as_secs_f32()),
                    MediaControlEvent::Seek(SeekDirection::Forward) => Command::SeekBy(10.0),
                    MediaControlEvent::Seek(SeekDirection::Backward) => Command::SeekBy(-10.0),
                    MediaControlEvent::SeekBy(SeekDirection::Forward, by) => Command::SeekBy(by.as_secs_f32()),
                    MediaControlEvent::SeekBy(SeekDirection::Backward, by) => Command::SeekBy(-by.as_secs_f32()),
                    MediaControlEvent::Raise => Command::Raise,
                    _ => return,
                };
                let _ = send.send(crate::app::Event::Media(command));
                wake();
            });
            attached.ok().map(|_| controls)
        });
        Media { controls }
    }

    pub fn show_song(&mut self, title: &str, artist: &str, album: &str, duration: f32) {
        if let Some(controls) = &mut self.controls {
            let _ = controls.set_metadata(MediaMetadata {
                title: Some(title),
                artist: Some(artist),
                album: (!album.is_empty()).then_some(album),
                duration: (duration > 0.0).then(|| Duration::from_secs_f32(duration)),
                cover_url: None,
            });
        }
    }

    pub fn show_state(&mut self, playing: bool, position: f32) {
        if let Some(controls) = &mut self.controls {
            let progress = Some(MediaPosition(Duration::from_secs_f32(position.max(0.0))));
            let _ = controls.set_playback(if playing { MediaPlayback::Playing { progress } } else { MediaPlayback::Paused { progress } });
        }
    }
}

// Where programs find the desktop's session bus: an address in the environment, or the
// usual socket in the user's runtime folder
fn session_bus_exists() -> bool {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|a| !a.is_empty()) {
        return true;
    }
    std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|dir| std::path::Path::new(&dir).join("bus").exists())
}
