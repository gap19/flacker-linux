//! A passive buffer probe: reads negotiated PCM without converting or modifying it.
use gst::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
use std::time::Instant;
#[derive(Default)]
pub struct Meter {
    pub enabled: AtomicBool,
    left: AtomicU32,
    right: AtomicU32,
    rms_left: AtomicU32,
    rms_right: AtomicU32,
    last: AtomicU64,
}
impl Meter {
    pub fn values(&self) -> [f32; 4] {
        [&self.left, &self.right, &self.rms_left, &self.rms_right]
            .map(|v| f32::from_bits(v.load(Ordering::Relaxed)))
    }
}
pub fn attach(sink: &gst::Element, meter: Arc<Meter>) {
    let Some(pad) = sink.static_pad("sink") else {
        return;
    };
    let start = Instant::now();
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        if !meter.enabled.load(Ordering::Relaxed) {
            return gst::PadProbeReturn::Ok;
        }
        let now = start.elapsed().as_millis() as u64;
        if now.saturating_sub(meter.last.load(Ordering::Relaxed)) < 80 {
            return gst::PadProbeReturn::Ok;
        }
        meter.last.store(now, Ordering::Relaxed);
        if let (Some(buffer), Some(caps)) = (info.buffer(), pad.current_caps()) {
            if let (Ok(data), Some(s)) = (buffer.map_readable(), caps.structure(0)) {
                if s.get::<String>("layout").is_ok_and(|s| s == "interleaved") {
                    if let (Ok(format), Ok(channels)) =
                        (s.get::<String>("format"), s.get::<i32>("channels"))
                    {
                        if let Some(values) = analyze(data.as_slice(), &format, channels as usize) {
                            for (a, v) in
                                [&meter.left, &meter.right, &meter.rms_left, &meter.rms_right]
                                    .iter()
                                    .zip(values)
                            {
                                a.store(v.to_bits(), Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        }
        gst::PadProbeReturn::Ok
    });
}
fn analyze(data: &[u8], format: &str, channels: usize) -> Option<[f32; 4]> {
    let width = match format {
        "S16LE" => 2,
        "S24LE" => 3,
        "S32LE" | "S24_32LE" | "F32LE" => 4,
        "F64LE" => 8,
        _ => return None,
    };
    if channels == 0 || channels > 64 {
        return None;
    }
    let frames = data.len() / (width * channels);
    if frames == 0 {
        return None;
    }
    let mut peak = [0f64; 2];
    let mut power = [0f64; 2];
    let mut n = 0;
    // At most 4096 frames every 80 ms, independent of sample rate and buffer length.
    for i in (0..frames).step_by(frames.div_ceil(4096)) {
        for c in 0..2 {
            let at = (i * channels + c.min(channels - 1)) * width;
            let b = &data[at..at + width];
            let value = match format {
                "S16LE" => i16::from_le_bytes(b.try_into().ok()?) as f64 / 32768.,
                "S24LE" => {
                    ((i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8) >> 8) as f64 / 8388608.
                }
                "S24_32LE" => i32::from_le_bytes(b.try_into().ok()?) as f64 / 8388608.,
                "S32LE" => i32::from_le_bytes(b.try_into().ok()?) as f64 / 2147483648.,
                "F32LE" => f32::from_le_bytes(b.try_into().ok()?) as f64,
                "F64LE" => f64::from_le_bytes(b.try_into().ok()?),
                _ => return None,
            };
            if value.is_finite() {
                peak[c] = peak[c].max(value.abs());
                power[c] += value * value;
            }
        }
        n += 1;
    }
    Some([
        peak[0] as f32,
        peak[1] as f32,
        (power[0] / n as f64).sqrt() as f32,
        (power[1] / n as f64).sqrt() as f32,
    ])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pcm_values_and_channels_are_real() {
        assert_eq!(
            analyze(&[0, 64, 0, 192], "S16LE", 2),
            Some([0.5, 0.5, 0.5, 0.5])
        );
        assert_eq!(
            analyze(&[0, 0, 128, 0, 0, 64], "S24LE", 2),
            Some([1., 0.5, 1., 0.5])
        );
        assert_eq!(analyze(&0.25f32.to_le_bytes(), "F32LE", 1), Some([0.25; 4]));
        assert!(analyze(&[], "S16LE", 2).is_none());
        assert!(analyze(&[0; 8], "S16LE", 0).is_none());
    }
}
