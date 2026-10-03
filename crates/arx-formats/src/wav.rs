//! WAV decoding. The game's sounds are RIFF/WAVE files that are mostly Microsoft ADPCM (format
//! tag 2), with some 8/16-bit PCM. Everything is decoded to interleaved 16-bit PCM.

use crate::reader::{Reader, Truncated};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WavError {
    #[error("file is truncated")]
    Truncated(#[from] Truncated),
    #[error("not a RIFF/WAVE file")]
    NotWav,
    #[error("missing {0} chunk")]
    MissingChunk(&'static str),
    #[error("unsupported format (tag {tag}, {bits} bits)")]
    Unsupported { tag: u16, bits: u16 },
    #[error("invalid {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    /// Interleaved samples.
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / (self.rate as f32 * self.channels as f32)
    }

    /// Standard 16-bit PCM WAV bytes, for handing to any ordinary decoder.
    pub fn to_wav_bytes(&self) -> Vec<u8> {
        let data_len = self.samples.len() * 2;
        let mut out = Vec::with_capacity(44 + data_len);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&self.channels.to_le_bytes());
        out.extend_from_slice(&self.rate.to_le_bytes());
        out.extend_from_slice(&(self.rate * self.channels as u32 * 2).to_le_bytes());
        out.extend_from_slice(&(self.channels * 2).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data_len as u32).to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

const ADAPTATION: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];

fn clamp16(v: i32) -> i32 {
    v.clamp(i16::MIN as i32, i16::MAX as i32)
}

struct AdpcmChannel {
    coef: (i32, i32),
    delta: i32,
    s1: i32,
    s2: i32,
}

impl AdpcmChannel {
    fn decode(&mut self, nibble: u8) -> i16 {
        let signed = if nibble >= 8 { nibble as i32 - 16 } else { nibble as i32 };
        let predicted = (self.s1 * self.coef.0 + self.s2 * self.coef.1) / 256 + signed * self.delta;
        let sample = clamp16(predicted);
        self.s2 = self.s1;
        self.s1 = sample;
        self.delta = (ADAPTATION[nibble as usize] * self.delta / 256).max(16);
        sample as i16
    }
}

fn decode_ms_adpcm(
    data: &[u8],
    channels: usize,
    block_align: usize,
    samples_per_block: usize,
    coefs: &[(i32, i32)],
) -> Result<Vec<i16>, WavError> {
    if channels == 0 || channels > 2 || block_align < 7 * channels || coefs.is_empty() {
        return Err(WavError::Invalid("ADPCM parameters"));
    }
    let mut out = Vec::new();
    for block in data.chunks(block_align) {
        let mut r = Reader::new(block);
        let header = 7 * channels;
        if block.len() < header {
            break;
        }
        let mut st = Vec::with_capacity(channels);
        let preds: Vec<usize> = (0..channels).map(|_| r.u8().map(usize::from)).collect::<Result<_, _>>()?;
        let deltas: Vec<i32> = (0..channels).map(|_| r.i16().map(i32::from)).collect::<Result<_, _>>()?;
        let s1: Vec<i32> = (0..channels).map(|_| r.i16().map(i32::from)).collect::<Result<_, _>>()?;
        let s2: Vec<i32> = (0..channels).map(|_| r.i16().map(i32::from)).collect::<Result<_, _>>()?;
        for c in 0..channels {
            let coef = *coefs.get(preds[c]).ok_or(WavError::Invalid("ADPCM predictor"))?;
            st.push(AdpcmChannel { coef, delta: deltas[c], s1: s1[c], s2: s2[c] });
        }
        // The two samples stored in the header come first (oldest first).
        let mut frames: Vec<Vec<i16>> = vec![Vec::new(); channels];
        for c in 0..channels {
            frames[c].push(st[c].s2 as i16);
            frames[c].push(st[c].s1 as i16);
        }
        let want = if samples_per_block > 0 { samples_per_block } else { usize::MAX };
        let body = &block[header..];
        'bytes: for &b in body {
            for (i, nibble) in [b >> 4, b & 15].into_iter().enumerate() {
                // Mono: both nibbles belong to the one channel; stereo: high = left, low = right.
                let c = if channels == 2 { i } else { 0 };
                if frames[c].len() >= want {
                    break 'bytes;
                }
                let s = st[c].decode(nibble);
                frames[c].push(s);
            }
        }
        let n = frames.iter().map(Vec::len).min().unwrap_or(0);
        for i in 0..n {
            for f in &frames {
                out.push(f[i]);
            }
        }
    }
    Ok(out)
}

/// Decode a WAV file to 16-bit PCM.
pub fn decode(file: &[u8]) -> Result<Pcm, WavError> {
    let mut r = Reader::new(file);
    if r.take(4)? != b"RIFF" {
        return Err(WavError::NotWav);
    }
    r.skip(4)?;
    if r.take(4)? != b"WAVE" {
        return Err(WavError::NotWav);
    }
    let mut fmt: Option<&[u8]> = None;
    let mut data: Option<&[u8]> = None;
    while r.remaining() >= 8 {
        let id = r.take(4)?;
        let size = r.u32()? as usize;
        let body = r.take(size.min(r.remaining()))?;
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        if size & 1 == 1 && r.remaining() > 0 {
            r.skip(1)?;
        }
    }
    let fmt = fmt.ok_or(WavError::MissingChunk("fmt"))?;
    let data = data.ok_or(WavError::MissingChunk("data"))?;
    let mut f = Reader::new(fmt);
    let tag = f.u16()?;
    let channels = f.u16()?;
    let rate = f.u32()?;
    f.skip(4)?; // byte rate
    let block_align = f.u16()? as usize;
    let bits = f.u16()?;

    let samples = match (tag, bits) {
        (1, 16) => data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(),
        (1, 8) => data.iter().map(|&b| ((b as i16) - 128) << 8).collect(),
        (2, 4) => {
            let cb_size = f.u16()? as usize;
            let samples_per_block = f.u16()? as usize;
            let n_coef = f.u16()? as usize;
            if cb_size < 4 + n_coef * 4 {
                return Err(WavError::Invalid("ADPCM header"));
            }
            let coefs = (0..n_coef)
                .map(|_| Ok::<_, Truncated>((i32::from(f.i16()?), i32::from(f.i16()?))))
                .collect::<Result<Vec<_>, _>>()?;
            decode_ms_adpcm(data, channels as usize, block_align, samples_per_block, &coefs)?
        }
        _ => return Err(WavError::Unsupported { tag, bits }),
    };
    if channels == 0 || rate == 0 {
        return Err(WavError::Invalid("channels or sample rate"));
    }
    Ok(Pcm { rate, channels, samples })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm16_wav(samples: &[i16]) -> Vec<u8> {
        Pcm { rate: 22050, channels: 1, samples: samples.to_vec() }.to_wav_bytes()
    }

    #[test]
    fn pcm_round_trip() {
        let src = [0i16, 1000, -1000, i16::MAX, i16::MIN];
        let pcm = decode(&pcm16_wav(&src)).unwrap();
        assert_eq!((pcm.rate, pcm.channels), (22050, 1));
        assert_eq!(pcm.samples, src);
    }

    #[test]
    fn ms_adpcm_known_block() {
        // One mono block: predictor 0 (coefs 256,0), delta 16, s1 = 100, s2 = 50, then nibbles
        // 0x1 and 0xF: sample = s1 + nibble * delta each time.
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&2u16.to_le_bytes()); // tag
        fmt.extend_from_slice(&1u16.to_le_bytes()); // channels
        fmt.extend_from_slice(&22050u32.to_le_bytes());
        fmt.extend_from_slice(&0u32.to_le_bytes());
        fmt.extend_from_slice(&8u16.to_le_bytes()); // block align
        fmt.extend_from_slice(&4u16.to_le_bytes()); // bits
        fmt.extend_from_slice(&(4u16 + 4).to_le_bytes()); // cbSize
        fmt.extend_from_slice(&4u16.to_le_bytes()); // samples per block
        fmt.extend_from_slice(&1u16.to_le_bytes()); // one coefficient pair
        fmt.extend_from_slice(&256i16.to_le_bytes());
        fmt.extend_from_slice(&0i16.to_le_bytes());
        let mut block = vec![0u8]; // predictor index
        block.extend_from_slice(&16i16.to_le_bytes()); // delta
        block.extend_from_slice(&100i16.to_le_bytes()); // sample1
        block.extend_from_slice(&50i16.to_le_bytes()); // sample2
        block.push(0x1F);
        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        wav.extend_from_slice(&fmt);
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(block.len() as u32).to_le_bytes());
        wav.extend_from_slice(&block);
        let pcm = decode(&wav).unwrap();
        // 50, 100, then 100 + 1*16 = 116, then the delta adapts (230*16/256 -> 16) and 0xF is -1: 116 - 16 = 100.
        assert_eq!(pcm.samples, [50, 100, 116, 100]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(decode(b"nope"), Err(_)));
        assert!(matches!(decode(&[0u8; 64]), Err(WavError::NotWav)));
    }
}
