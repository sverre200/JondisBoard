//! A small WAV reader: PCM 8/16/24/32-bit and float 32/64, plain or WAVE_FORMAT_EXTENSIBLE.
//! 48 kHz mono/stereo files are read bit-exactly; everything else goes through ffmpeg
//! (see `decode.rs`), never a home-made resampler or downmix.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    Int,
    Float,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavInfo {
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub format: SampleFormat,
    pub data_offset: u64,
    pub data_len: u64,
}

impl WavInfo {
    pub fn frames(&self) -> u64 {
        let bpf = (self.bits as u64 / 8) * self.channels as u64;
        if bpf == 0 {
            0
        } else {
            self.data_len / bpf
        }
    }
    pub fn duration(&self) -> f64 {
        if self.rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.rate as f64
        }
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Read just the header (fast: used to show durations without decoding).
pub fn read_info(path: &Path) -> io::Result<WavInfo> {
    let mut f = File::open(path)?;
    parse_header(&mut f)
}

fn parse_header<R: Read + Seek>(f: &mut R) -> io::Result<WavInfo> {
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr)?;
    if &hdr[0..4] != b"RIFF" || &hdr[8..12] != b"WAVE" {
        return Err(bad("not a WAV file"));
    }
    let mut fmt: Option<(u16, u16, u32, u16, Option<u16>)> = None;
    loop {
        let mut ch = [0u8; 8];
        if f.read_exact(&mut ch).is_err() {
            return Err(bad("WAV has no data chunk"));
        }
        let id = [ch[0], ch[1], ch[2], ch[3]];
        let len = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        match &id {
            b"fmt " => {
                let mut b = vec![0u8; len.min(64) as usize];
                f.read_exact(&mut b)?;
                if len > 64 {
                    f.seek(SeekFrom::Current((len - 64) as i64))?;
                }
                if b.len() < 16 {
                    return Err(bad("short fmt chunk"));
                }
                let tag = u16::from_le_bytes([b[0], b[1]]);
                let channels = u16::from_le_bytes([b[2], b[3]]);
                let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                let bits = u16::from_le_bytes([b[14], b[15]]);
                // WAVE_FORMAT_EXTENSIBLE: the real format is the first 2 bytes of the sub-format GUID
                let sub = if tag == 0xFFFE && b.len() >= 26 { Some(u16::from_le_bytes([b[24], b[25]])) } else { None };
                fmt = Some((tag, channels, rate, bits, sub));
                if len % 2 == 1 {
                    f.seek(SeekFrom::Current(1))?;
                }
            }
            b"data" => {
                let (tag, channels, rate, bits, sub) = fmt.ok_or_else(|| bad("WAV data before fmt"))?;
                let real = sub.unwrap_or(tag);
                let format = match (real, bits) {
                    (1, 8 | 16 | 24 | 32) => SampleFormat::Int,
                    (3, 32 | 64) => SampleFormat::Float,
                    _ => return Err(bad("unsupported WAV encoding")),
                };
                if channels == 0 || rate == 0 {
                    return Err(bad("bad WAV header"));
                }
                let data_offset = f.stream_position()?;
                let file_len = f.seek(SeekFrom::End(0))?;
                // a streaming writer may leave the length at 0 or 0xFFFFFFFF: use what's there
                let avail = file_len.saturating_sub(data_offset);
                let data_len = if len == 0 || len == 0xFFFF_FFFF || len > avail { avail } else { len };
                return Ok(WavInfo { rate, channels, bits, format, data_offset, data_len });
            }
            _ => {
                f.seek(SeekFrom::Current((len + (len & 1)) as i64))?;
            }
        }
    }
}

/// Decoded audio at 48 kHz, interleaved stereo. 16-bit sources stay 16-bit in memory (half the
/// RAM); the mixer converts on the fly with the same exact scale (x / 32768).
pub enum Samples {
    I16(Box<[i16]>),
    F32(Box<[f32]>),
}

impl Samples {
    pub fn frames(&self) -> usize {
        match self {
            Samples::I16(v) => v.len() / 2,
            Samples::F32(v) => v.len() / 2,
        }
    }
    pub fn bytes(&self) -> usize {
        match self {
            Samples::I16(v) => v.len() * 2,
            Samples::F32(v) => v.len() * 4,
        }
    }
    /// Stereo frame i as f32.
    #[inline]
    pub fn frame(&self, i: usize) -> (f32, f32) {
        match self {
            Samples::I16(v) => (v[2 * i] as f32 / 32768.0, v[2 * i + 1] as f32 / 32768.0),
            Samples::F32(v) => (v[2 * i], v[2 * i + 1]),
        }
    }
}

/// Is this a WAV that can be read as is (48 kHz, 1-2 channels)?
pub fn direct(info: &WavInfo) -> bool {
    info.rate == 48000 && (1..=2).contains(&info.channels)
}

/// Decode a 48 kHz mono/stereo WAV bit-exactly. Mono becomes the same signal on both sides.
pub fn decode(path: &Path, info: &WavInfo) -> io::Result<Samples> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(info.data_offset))?;
    let mut raw = vec![0u8; info.data_len as usize];
    f.read_exact(&mut raw)?;
    Ok(decode_bytes(&raw, info))
}

pub fn decode_bytes(raw: &[u8], info: &WavInfo) -> Samples {
    let ch = info.channels as usize;
    let frames = info.frames() as usize;
    let stereo = |get: &dyn Fn(usize) -> f32| -> Box<[f32]> {
        let mut out = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let l = get(i * ch);
            let r = if ch >= 2 { get(i * ch + 1) } else { l };
            out.push(l);
            out.push(r);
        }
        out.into_boxed_slice()
    };
    match (info.format, info.bits) {
        (SampleFormat::Int, 16) => {
            let s = |i: usize| i16::from_le_bytes([raw[2 * i], raw[2 * i + 1]]);
            let mut out = Vec::with_capacity(frames * 2);
            for i in 0..frames {
                let l = s(i * ch);
                out.push(l);
                out.push(if ch >= 2 { s(i * ch + 1) } else { l });
            }
            Samples::I16(out.into_boxed_slice())
        }
        (SampleFormat::Int, 8) => Samples::F32(stereo(&|i| (raw[i] as f32 - 128.0) / 128.0)),
        (SampleFormat::Int, 24) => Samples::F32(stereo(&|i| {
            let v = i32::from_le_bytes([0, raw[3 * i], raw[3 * i + 1], raw[3 * i + 2]]) >> 8;
            v as f32 / 8_388_608.0
        })),
        (SampleFormat::Int, _) => Samples::F32(stereo(&|i| {
            let v = i32::from_le_bytes([raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]]);
            (v as f64 / 2_147_483_648.0) as f32
        })),
        (SampleFormat::Float, 64) => Samples::F32(stereo(&|i| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&raw[8 * i..8 * i + 8]);
            f64::from_le_bytes(b) as f32
        })),
        (SampleFormat::Float, _) => Samples::F32(stereo(&|i| {
            f32::from_le_bytes([raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]])
        })),
    }
}

/// Write a 48 kHz stereo float WAV (diagnostics and tests).
pub fn write_f32(path: &Path, rate: u32, channels: u16, data: &[f32]) -> io::Result<()> {
    let data_len = (data.len() * 4) as u32;
    let mut b = Vec::with_capacity(44 + data.len() * 4);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * channels as u32 * 4).to_le_bytes());
    b.extend_from_slice(&(channels * 4).to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for x in data {
        b.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(path, b)
}

/// Write a 16-bit PCM WAV (tests, generated tones).
pub fn write_i16(path: &Path, rate: u32, channels: u16, data: &[i16]) -> io::Result<()> {
    let data_len = (data.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    b.extend_from_slice(&(channels * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for x in data {
        b.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(path, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("jb_wav_{}_{name}", std::process::id()))
    }

    #[test]
    fn pcm16_mono_is_bit_exact_and_doubled() {
        let p = tmp("m16.wav");
        let data: Vec<i16> = vec![0, 1, -1, 32767, -32768, 1234];
        write_i16(&p, 48000, 1, &data).unwrap();
        let info = read_info(&p).unwrap();
        assert_eq!((info.rate, info.channels, info.bits, info.frames()), (48000, 1, 16, 6));
        assert!(direct(&info));
        let s = decode(&p, &info).unwrap();
        assert_eq!(s.frames(), 6);
        assert_eq!(s.frame(3), (32767.0 / 32768.0, 32767.0 / 32768.0));
        assert_eq!(s.frame(4), (-1.0, -1.0));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn float_stereo_roundtrip_and_other_rates_flagged() {
        let p = tmp("f32.wav");
        let data = [0.5f32, -0.25, 1.5, -2.0];
        write_f32(&p, 44100, 2, &data).unwrap();
        let info = read_info(&p).unwrap();
        assert!(!direct(&info));
        assert_eq!(info.format, SampleFormat::Float);
        let s = decode(&p, &info).unwrap();
        assert_eq!(s.frame(0), (0.5, -0.25));
        assert_eq!(s.frame(1), (1.5, -2.0));
        assert!((info.duration() - 2.0 / 44100.0).abs() < 1e-9);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn pcm24_values() {
        let info = WavInfo { rate: 48000, channels: 1, bits: 24, format: SampleFormat::Int, data_offset: 0, data_len: 6 };
        // 0x7fffff and 0x800000
        let s = decode_bytes(&[0xff, 0xff, 0x7f, 0x00, 0x00, 0x80], &info);
        assert_eq!(s.frame(0).0, 8_388_607.0 / 8_388_608.0);
        assert_eq!(s.frame(1).0, -1.0);
    }

    #[test]
    fn not_a_wav_is_an_error() {
        let p = tmp("bad.wav");
        std::fs::write(&p, b"ID3 this is an mp3").unwrap();
        assert!(read_info(&p).is_err());
        let _ = std::fs::remove_file(p);
    }
}
