//! Custom rodio `Source` for OGG-encapsulated Opus audio.
//!
//! rodio 0.19 + Symphonia does not include an Opus decoder, but Matrix
//! voice notes (m.audio messages from clients like Element, mautrix-whatsapp,
//! mautrix-telegram, …) are virtually always shipped as OGG/Opus. This
//! module wraps `libopus` (via the `opus` crate) and `ogg` to produce a
//! `rodio::Source<i16>` that can be appended to a `Sink`.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ogg::reading::PacketReader;
use soundtouch::{Setting, SoundTouch};

pub struct OpusSource {
    decoder: opus::Decoder,
    reader: PacketReader<Cursor<Vec<u8>>>,
    pcm: Vec<i16>,
    pcm_pos: usize,
    sample_rate: u32,
    channels: u16,
    pre_skip_remaining: u32,
}

impl OpusSource {
    /// Parse the OGG header packets, initialise the Opus decoder, and
    /// return a `Source` ready to be appended to a `Sink`.
    pub fn try_from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        let mut reader = PacketReader::new(Cursor::new(bytes));

        let head = reader
            .read_packet()
            .map_err(|e| format!("ogg read: {e}"))?
            .ok_or("missing OpusHead packet")?;
        if head.data.len() < 19 || &head.data[..8] != b"OpusHead" {
            return Err("not an Opus stream (OpusHead magic mismatch)".into());
        }
        let channels = head.data[9] as u16;
        let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as u32;

        // OpusTags packet is mandatory but irrelevant for playback.
        let _tags = reader
            .read_packet()
            .map_err(|e| format!("ogg tags: {e}"))?;

        let opus_channels = match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            n => return Err(format!("unsupported Opus channel count: {n}")),
        };
        // Opus' internal rates are 8/12/16/24/48 kHz; we always decode at
        // 48 kHz and let rodio resample to the device rate.
        let sample_rate = 48_000;
        let decoder = opus::Decoder::new(sample_rate, opus_channels)
            .map_err(|e| format!("opus init: {e}"))?;

        Ok(Self {
            decoder,
            reader,
            pcm: Vec::new(),
            pcm_pos: 0,
            sample_rate,
            channels,
            pre_skip_remaining: pre_skip,
        })
    }

    /// Decode the next OGG packet into `self.pcm`, applying pre-skip if any.
    /// Returns false when the stream is exhausted.
    fn refill(&mut self) -> bool {
        // Largest Opus frame is 120 ms = 5760 samples per channel @ 48 kHz.
        const MAX_FRAME_SAMPLES: usize = 5760;

        loop {
            let packet = match self.reader.read_packet() {
                Ok(Some(p)) => p,
                Ok(None) | Err(_) => return false,
            };
            self.pcm
                .resize(MAX_FRAME_SAMPLES * self.channels as usize, 0);
            let n = match self.decoder.decode(&packet.data, &mut self.pcm, false) {
                Ok(n) => n,
                Err(_) => continue, // skip un-decodable packet
            };
            self.pcm.truncate(n * self.channels as usize);
            self.pcm_pos = 0;

            // Discard pre-skip samples (encoder lookahead).
            if self.pre_skip_remaining > 0 {
                let skip = (self.pre_skip_remaining as usize).min(n);
                self.pcm_pos = skip * self.channels as usize;
                self.pre_skip_remaining -= skip as u32;
                if self.pcm_pos >= self.pcm.len() {
                    continue; // whole frame was pre-skip; load another
                }
            }
            return true;
        }
    }
}

impl Iterator for OpusSource {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        if self.pcm_pos >= self.pcm.len() && !self.refill() {
            return None;
        }
        let s = self.pcm[self.pcm_pos];
        self.pcm_pos += 1;
        Some(s)
    }
}

impl rodio::Source for OpusSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// `f32` packed into an `AtomicU32` via `to_bits` / `from_bits`. Used to
/// share the playback tempo (1.0× = realtime) between the UI thread and
/// the [`StretchSource`] running inside the audio thread.
pub type SharedTempo = Arc<AtomicU32>;

pub fn make_shared_tempo() -> SharedTempo {
    Arc::new(AtomicU32::new(1.0f32.to_bits()))
}

pub fn store_tempo(t: &SharedTempo, value: f32) {
    t.store(value.to_bits(), Ordering::Relaxed);
}

pub fn load_tempo(t: &SharedTempo) -> f32 {
    f32::from_bits(t.load(Ordering::Relaxed))
}

/// Source-time playback position in seconds, i.e. how far into the original
/// audio we've consumed (independent of tempo). Same `AtomicU32` packing
/// trick as the tempo. Wall-clock position from `Sink::get_pos` is wrong
/// once the user changes speed.
pub type SharedPosition = Arc<AtomicU32>;

pub fn make_shared_position() -> SharedPosition {
    Arc::new(AtomicU32::new(0.0f32.to_bits()))
}

pub fn store_position(p: &SharedPosition, value: f32) {
    p.store(value.to_bits(), Ordering::Relaxed);
}

pub fn load_position(p: &SharedPosition) -> f32 {
    f32::from_bits(p.load(Ordering::Relaxed))
}

/// Wraps any `Source<Item = i16>` with SoundTouch tempo (time-stretching
/// at constant pitch). The tempo is read each refill from a shared atomic
/// so the UI thread can change speed mid-playback without re-creating the
/// source.
pub struct StretchSource<S> {
    inner: S,
    sample_rate: u32,
    channels: u16,
    st: SoundTouch,
    tempo: SharedTempo,
    last_tempo: f32,
    out_buf: VecDeque<i16>,
    inner_done: bool,
    flushed: bool,
    /// Frames pulled from the inner source so far. `frames / sample_rate`
    /// is the source-time playback position published to `position`.
    frames_consumed: u64,
    position: SharedPosition,
}

impl<S: rodio::Source<Item = i16>> StretchSource<S> {
    pub fn new(inner: S, tempo: SharedTempo, position: SharedPosition) -> Self {
        let sample_rate = inner.sample_rate();
        let channels = inner.channels();
        let mut st = SoundTouch::new();
        st.set_channels(channels as u32)
            .set_sample_rate(sample_rate)
            .set_tempo(load_tempo(&tempo) as f64)
            // Quickseek trades a bit of quality for ~2× faster processing,
            // which keeps CPU low for stutter-free playback in a TUI.
            .set_setting(Setting::UseQuickseek, 1);
        store_position(&position, 0.0);
        Self {
            inner,
            sample_rate,
            channels,
            st,
            last_tempo: load_tempo(&tempo),
            tempo,
            out_buf: VecDeque::new(),
            inner_done: false,
            flushed: false,
            frames_consumed: 0,
            position,
        }
    }

    fn refill(&mut self) {
        // Pick up tempo changes between refills.
        let cur = load_tempo(&self.tempo);
        if (cur - self.last_tempo).abs() > 1e-3 {
            self.st.set_tempo(cur as f64);
            self.last_tempo = cur;
        }

        // Pull a chunk of input frames; convert i16 → f32 in [-1, 1].
        const CHUNK_FRAMES: usize = 1024;
        let frame_samples = CHUNK_FRAMES * self.channels as usize;
        if !self.inner_done {
            let mut input: Vec<f32> = Vec::with_capacity(frame_samples);
            for _ in 0..frame_samples {
                match self.inner.next() {
                    Some(s) => input.push(s as f32 / i16::MAX as f32),
                    None => {
                        self.inner_done = true;
                        break;
                    }
                }
            }
            if !input.is_empty() {
                let frames_pulled = input.len() / self.channels as usize;
                self.st.put_samples(&input, frames_pulled);
                self.frames_consumed += frames_pulled as u64;
                let pos = self.frames_consumed as f32 / self.sample_rate as f32;
                store_position(&self.position, pos);
            }
        }
        if self.inner_done && !self.flushed {
            self.st.flush();
            self.flushed = true;
        }

        // Drain everything available from SoundTouch into our output buffer.
        const OUT_CAP_FRAMES: usize = 2048;
        let mut tmp = vec![0f32; OUT_CAP_FRAMES * self.channels as usize];
        loop {
            let n = self.st.receive_samples(tmp.as_mut_slice(), OUT_CAP_FRAMES);
            if n == 0 {
                break;
            }
            for v in &tmp[..n * self.channels as usize] {
                let s = (v * i16::MAX as f32)
                    .clamp(i16::MIN as f32, i16::MAX as f32)
                    as i16;
                self.out_buf.push_back(s);
            }
        }
    }
}

impl<S: rodio::Source<Item = i16>> Iterator for StretchSource<S> {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        loop {
            if let Some(s) = self.out_buf.pop_front() {
                return Some(s);
            }
            if self.inner_done && self.flushed {
                return None;
            }
            self.refill();
        }
    }
}

impl<S: rodio::Source<Item = i16>> rodio::Source for StretchSource<S> {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
