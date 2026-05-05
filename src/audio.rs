//! Custom rodio `Source` for OGG-encapsulated Opus audio.
//!
//! rodio 0.19 + Symphonia does not include an Opus decoder, but Matrix
//! voice notes (m.audio messages from clients like Element, mautrix-whatsapp,
//! mautrix-telegram, …) are virtually always shipped as OGG/Opus. This
//! module wraps `libopus` (via the `opus` crate) and `ogg` to produce a
//! `rodio::Source<i16>` that can be appended to a `Sink`.

use std::io::Cursor;
use std::time::Duration;

use ogg::reading::PacketReader;

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
