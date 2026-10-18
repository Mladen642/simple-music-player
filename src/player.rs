// Playing a song through the speakers (rodio). One song at a time; the app decides
// what comes next.
use std::fs::File;
use std::path::Path;
use std::time::Duration;

pub struct Player {
    _device: Option<rodio::MixerDeviceSink>, // the sound card (kept open while the app runs)
    output: Option<rodio::Player>,
    pub problem: Option<String>,             // why there's no sound, if there isn't
    loaded: bool,
    finished: bool,
    volume: f32,
    gain: f32,                               // "Even volume" for this song (1 = as it is)
    pub recent: crate::analysis::Recent,     // the last moment of sound, for the equalizer
}

impl Player {
    pub fn new() -> Player {
        match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(mut device) => {
                device.log_on_drop(false);
                let output = rodio::Player::connect_new(device.mixer());
                Player { _device: Some(device), output: Some(output), problem: None, loaded: false, finished: false, volume: 0.8, gain: 1.0, recent: Default::default() }
            }
            Err(error) => Player {
                _device: None,
                output: None,
                problem: Some(format!("No sound device found ({error}).")),
                loaded: false,
                finished: false,
                volume: 0.8,
                gain: 1.0,
                recent: Default::default(),
            },
        }
    }

    // Starts a song from the beginning
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        let output = self.output.as_ref().ok_or_else(|| self.problem.clone().unwrap_or_default())?;
        output.clear();
        self.loaded = false;
        self.finished = false;
        let file = File::open(path).map_err(|e| e.to_string())?;
        let decoder = rodio::Decoder::try_from(file).map_err(|e| e.to_string())?;
        self.recent.clear();
        output.append(crate::analysis::Tap::new(decoder, self.recent.clone()));
        output.play();
        self.loaded = true;
        Ok(())
    }

    pub fn play(&self) {
        if let Some(output) = &self.output {
            output.play();
        }
    }

    pub fn pause(&self) {
        if let Some(output) = &self.output {
            output.pause();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.loaded && !self.finished && self.output.as_ref().is_some_and(|o| !o.is_paused() && !o.empty())
    }

    pub fn has_song(&self) -> bool {
        self.loaded
    }

    // The song played to its end (pressing play starts it again)
    pub fn ended(&self) -> bool {
        self.loaded && self.finished
    }

    pub fn position(&self) -> f32 {
        self.output.as_ref().map_or(0.0, |o| o.get_pos().as_secs_f32())
    }

    pub fn seek(&self, seconds: f32) {
        if let Some(output) = &self.output {
            let _ = output.try_seek(Duration::from_secs_f32(seconds.max(0.0)));
        }
    }

    // volume: the slider (0 to 1); a curve makes the slider feel even, like most players
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        self.apply_volume();
    }

    // "Even volume": how much this song is turned up or down
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
        self.apply_volume();
    }

    fn apply_volume(&self) {
        if let Some(output) = &self.output {
            output.set_volume(self.volume * self.volume * self.gain);
        }
    }

    // True once, when the song has played to its end
    pub fn just_finished(&mut self) -> bool {
        if self.loaded && !self.finished && self.output.as_ref().is_some_and(|o| o.empty()) {
            self.finished = true;
            return true;
        }
        false
    }
}
