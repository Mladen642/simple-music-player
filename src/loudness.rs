// "Even volume": every song at about the same loudness. Each song is measured once,
// the way streaming services do it (EBU R128): how loud it sounds overall (LUFS) and
// its loudest moment (peak). A loud song is turned down to the target; a quiet one is
// turned up, but only as far as its peaks allow, so it never distorts. The files are
// never changed. Measurements are kept in loudness.json (the old player's too).
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

const TARGET: f32 = -14.0;  // LUFS, the level Spotify and YouTube play songs at
const MAX_BOOST: f32 = 9.0; // dB, the most a quiet song is turned up

// [LUFS, peak in dBFS], or None when the song couldn't be measured
pub type Measured = Option<(f32, Option<f32>)>;

pub fn load(data: &Path) -> HashMap<String, Measured> {
    let Ok(text) = std::fs::read_to_string(data.join("loudness.json")) else { return HashMap::new() };
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return HashMap::new() };
    let Some(songs) = value.get("songs").and_then(Value::as_object) else { return HashMap::new() };
    songs
        .iter()
        .map(|(id, value)| {
            let measured = value.as_array().and_then(|pair| {
                let lufs = pair.first()?.as_f64()? as f32;
                Some((lufs, pair.get(1).and_then(Value::as_f64).map(|p| p as f32)))
            });
            (id.clone(), measured)
        })
        .collect()
}

pub fn save(data: &Path, known: &HashMap<String, Measured>) {
    let songs: Map<String, Value> = known
        .iter()
        .map(|(id, measured)| (id.clone(), measured.map_or(Value::Null, |(lufs, peak)| serde_json::json!([lufs, peak]))))
        .collect();
    let text = serde_json::to_string_pretty(&serde_json::json!({ "version": 1, "songs": songs })).unwrap_or_default();
    let _ = crate::store::write_safely(&data.join("loudness.json"), &text);
}

// How much to turn a song up or down (1 = as it is)
pub fn gain(measured: &Measured) -> f32 {
    let Some((lufs, peak)) = measured else { return 1.0 };
    let mut db = TARGET - lufs;
    if db > 0.0 {
        db = db.min(MAX_BOOST).min(-1.0 - peak.unwrap_or(0.0)).max(0.0); // up, but keep 1 dB below the top
    }
    10f32.powf(db / 20.0)
}

// Songs not measured yet play at the middle level of those that are, which is much
// closer than playing them as they are (most songs get turned down)
pub fn typical_gain(known: &HashMap<String, Measured>) -> f32 {
    let mut gains: Vec<f32> = known.values().filter(|m| m.is_some()).map(gain).collect();
    if gains.is_empty() {
        return 1.0;
    }
    gains.sort_by(f32::total_cmp);
    gains[gains.len() / 2]
}

// Decodes the whole song and measures it (about a second for a four-minute song)
pub fn measure(path: &Path) -> Measured {
    use rodio::Source;
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = rodio::Decoder::try_from(file).ok()?;
    let channels = decoder.channels().get() as u32;
    let rate = decoder.sample_rate().get();
    let mode = ebur128::Mode::I | ebur128::Mode::SAMPLE_PEAK;
    let mut meter = ebur128::EbuR128::new(channels, rate, mode).ok()?;
    let mut chunk = Vec::with_capacity(4096 * channels as usize);
    loop {
        chunk.clear();
        chunk.extend(decoder.by_ref().take(4096 * channels as usize));
        if chunk.len() < channels as usize {
            break;
        }
        let whole = chunk.len() - chunk.len() % channels as usize;
        meter.add_frames_f32(&chunk[..whole]).ok()?;
    }
    let lufs = meter.loudness_global().ok()?;
    if !lufs.is_finite() {
        return Some((-70.0, None)); // silent
    }
    let peak = (0..channels).filter_map(|c| meter.sample_peak(c).ok()).fold(0.0f64, f64::max);
    let peak_db = (peak > 0.0).then(|| (20.0 * peak.log10()) as f32);
    Some((((lufs * 10.0).round() / 10.0) as f32, peak_db.map(|p| (p * 10.0).round() / 10.0)))
}

// Measures songs one by one in the background; the song that starts playing goes first
pub struct Meter {
    line: Arc<(Mutex<VecDeque<(String, PathBuf)>>, Condvar)>,
}

impl Meter {
    pub fn start(send: Sender<crate::app::Event>, wake: impl Fn() + Send + 'static) -> Meter {
        let line: Arc<(Mutex<VecDeque<(String, PathBuf)>>, Condvar)> = Arc::default();
        let worker = line.clone();
        std::thread::spawn(move || loop {
            let (id, path) = {
                let (jobs, ready) = &*worker;
                let mut jobs = jobs.lock().unwrap();
                loop {
                    if let Some(job) = jobs.pop_front() {
                        break job;
                    }
                    jobs = ready.wait(jobs).unwrap();
                }
            };
            let measured = measure(&path);
            if send.send(crate::app::Event::Loudness(id, measured)).is_err() {
                return;
            }
            wake();
        });
        Meter { line }
    }

    pub fn request(&self, id: String, path: PathBuf, urgent: bool) {
        let (jobs, ready) = &*self.line;
        let mut jobs = jobs.lock().unwrap();
        if urgent {
            jobs.retain(|(other, _)| *other != id);
            jobs.push_front((id, path));
        } else if !jobs.iter().any(|(other, _)| *other == id) {
            jobs.push_back((id, path));
        }
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn gains() {
        assert!((super::gain(&Some((-8.0, Some(0.0)))) - 0.501).abs() < 0.01); // loud: down 6 dB
        assert_eq!(super::gain(&Some((-18.0, Some(-0.5)))), 1.0); // quiet, but no room to go up
        assert!((super::gain(&Some((-20.0, Some(-12.0)))) - 1.995).abs() < 0.01); // quiet with room: up 6 dB
        assert_eq!(super::gain(&None), 1.0);
    }
}
