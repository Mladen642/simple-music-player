// The equalizer under the song's name. The music passes through a Tap on its way to
// the speakers, which keeps the last moment of sound; every frame, a Fourier transform
// splits that moment into frequencies, and each bar shows how loud its range is.
use rodio::Source;
use rustfft::{FftPlanner, num_complex::Complex};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WINDOW: usize = 2048; // samples per analysis (about 46 ms of music)
pub const BARS: usize = 40;

// The last WINDOW samples (mono), shared between the sound thread and the screen
#[derive(Clone, Default)]
pub struct Recent {
    inner: Arc<Mutex<Shared>>,
}

#[derive(Default)]
struct Shared {
    samples: Vec<f32>,
    sample_rate: u32,
}

impl Recent {
    fn push(&self, chunk: &[f32], sample_rate: u32) {
        if let Ok(mut shared) = self.inner.lock() {
            shared.sample_rate = sample_rate;
            shared.samples.extend_from_slice(chunk);
            let extra = shared.samples.len().saturating_sub(WINDOW);
            shared.samples.drain(..extra);
        }
    }

    pub fn clear(&self) {
        if let Ok(mut shared) = self.inner.lock() {
            shared.samples.clear();
        }
    }
}

// Passes the music through unchanged, keeping a copy of it (mixed down to mono)
pub struct Tap<S> {
    inner: S,
    recent: Recent,
    chunk: Vec<f32>,
    channel: u16,
    sum: f32,
}

impl<S: Source> Tap<S> {
    pub fn new(inner: S, recent: Recent) -> Tap<S> {
        Tap { inner, recent, chunk: Vec::with_capacity(512), channel: 0, sum: 0.0 }
    }
}

impl<S: Source> Iterator for Tap<S> {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<rodio::Sample> {
        let sample = self.inner.next()?;
        let channels = self.inner.channels().get();
        self.sum += sample;
        self.channel += 1;
        if self.channel >= channels {
            self.chunk.push(self.sum / channels as f32);
            self.channel = 0;
            self.sum = 0.0;
            if self.chunk.len() >= 512 {
                // handed over in pieces, so the sound thread rarely waits for the lock
                self.recent.push(&self.chunk, self.inner.sample_rate().get());
                self.chunk.clear();
            }
        }
        Some(sample)
    }
}

impl<S: Source> Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.recent.clear();
        self.inner.try_seek(position)
    }
}

// Turns the recent sound into bar heights (0 to 1), smoothed so the bars rise quickly
// and fall slowly, like the web version's
pub struct Spectrum {
    fft: Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,             // a Hann window, worked out once
    buffer: Vec<Complex<f32>>,    // the moment being analysed (used again every frame)
    scratch: Vec<Complex<f32>>,
    pub levels: [f32; BARS],
    peak: f32, // the loudest bar lately: quiet songs still fill the bars
}

impl Default for Spectrum {
    fn default() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(WINDOW);
        let zero = Complex { re: 0.0, im: 0.0 };
        Spectrum {
            window: (0..WINDOW).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (WINDOW - 1) as f32).cos()).collect(),
            buffer: vec![zero; WINDOW],
            scratch: vec![zero; fft.get_inplace_scratch_len()],
            fft,
            levels: [0.0; BARS],
            peak: 0.3,
        }
    }
}

impl Spectrum {
    // Returns true while the bars are still moving
    pub fn update(&mut self, recent: &Recent, playing: bool) -> bool {
        // The recent sound goes straight into the buffer, through a Hann window (so the
        // edges of the moment don't add false frequencies)
        let rate = match recent.inner.lock() {
            Ok(shared) if playing && shared.samples.len() == WINDOW => {
                for ((slot, sample), w) in self.buffer.iter_mut().zip(&shared.samples).zip(&self.window) {
                    *slot = Complex { re: sample * w, im: 0.0 };
                }
                Some(shared.sample_rate.max(1))
            }
            _ => None,
        };
        let mut target = [0.0f32; BARS];
        if let Some(rate) = rate {
            self.fft.process_with_scratch(&mut self.buffer, &mut self.scratch);
            let buffer = &self.buffer;
            let bin_hz = rate as f32 / WINDOW as f32;
            // Higher bars cover wider ranges, the way we hear pitch (40 Hz to 14 kHz)
            for (bar, level) in target.iter_mut().enumerate() {
                let low = 40.0 * (14000.0f32 / 40.0).powf(bar as f32 / BARS as f32);
                let high = 40.0 * (14000.0f32 / 40.0).powf((bar + 1) as f32 / BARS as f32);
                let (first, last) = ((low / bin_hz) as usize, ((high / bin_hz) as usize).max((low / bin_hz) as usize + 1));
                let bins = &buffer[first.min(WINDOW / 2)..last.min(WINDOW / 2)];
                let power = bins.iter().map(|c| c.norm()).fold(0.0f32, f32::max) / (WINDOW as f32 / 4.0);
                let db = 20.0 * power.max(1e-9).log10();
                *level = ((db + 70.0) / 60.0).clamp(0.0, 1.0); // -70 dB (silent) to -10 dB (loud)
            }
            let loudest = target.iter().copied().fold(0.0f32, f32::max);
            self.peak = (self.peak * 0.995).max(loudest).max(0.25);
            for level in &mut target {
                *level = (*level / self.peak).min(1.0).powf(1.4);
            }
        }
        let mut moving = false;
        for (level, target) in self.levels.iter_mut().zip(target) {
            let speed = if target > *level { 0.6 } else { 0.18 };
            *level += (target - *level) * speed;
            if *level > 0.002 {
                moving = true;
            } else {
                *level = 0.0;
            }
        }
        moving
    }
}
