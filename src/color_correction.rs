use crate::db::models::ImageRecord;

/// One Simplest Color Balance correction (Limare, Lisani, Morel, Petro, Sbert — IPOL 2011): a
/// low/high clip point per RGB channel that the pixel pipeline stretches linearly to 0-255.
/// Mirrors `ImageRecord::rotation`'s pattern of "a small numeric summary of a non-destructive
/// transform, applied at render/export time" — computed once by the Auto Correct button, stored
/// in the database, and replayed at both view time and recompressed-export time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorCorrectionParams {
    pub black: [u8; 3],
    pub white: [u8; 3],
}

/// Fraction of a channel's pixels clipped from EACH tail before stretching the rest to the full
/// range — Limare et al.'s default operating point (1% low + 1% high = 2% total).
pub const DEFAULT_CLIP_FRACTION: f64 = 0.01;

/// A 256-bucket histogram of one channel's values across every pixel in an interleaved buffer
/// (e.g. RGB or RGBA). `channels` is the buffer's bytes-per-pixel (3 or 4); `channel_index`
/// selects which byte of each pixel to bucket (0=R, 1=G, 2=B).
pub fn channel_histogram(pixels: &[u8], channels: usize, channel_index: usize) -> [u32; 256] {
    let mut histogram = [0u32; 256];
    let mut i = channel_index;
    while i < pixels.len() {
        histogram[pixels[i] as usize] += 1;
        i += channels;
    }
    histogram
}

/// The (black, white) clip points for one channel: the smallest value whose cumulative count
/// from 0 exceeds `clip_fraction` of the total pixel count, and the largest value whose
/// cumulative count from 255 likewise exceeds it. Falls back to a no-op stretch (0, 255) for an
/// empty histogram, and guards against `black > white` (a near-uniform channel, where both tails'
/// clip thresholds land past each other) by also falling back to a no-op in that case.
pub fn clip_points(histogram: &[u32; 256], clip_fraction: f64) -> (u8, u8) {
    let total: u32 = histogram.iter().sum();
    if total == 0 {
        return (0, 255);
    }
    let clip_count = (total as f64 * clip_fraction) as u32;

    let mut black = 0u8;
    let mut cumulative = 0u32;
    for (value, &count) in histogram.iter().enumerate() {
        cumulative += count;
        if cumulative > clip_count {
            black = value as u8;
            break;
        }
    }

    let mut white = 255u8;
    let mut cumulative = 0u32;
    for (value, &count) in histogram.iter().enumerate().rev() {
        cumulative += count;
        if cumulative > clip_count {
            white = value as u8;
            break;
        }
    }

    if black >= white {
        (0, 255)
    } else {
        (black, white)
    }
}

/// Computes `ColorCorrectionParams` from an interleaved RGB(A) pixel buffer via `clip_points`
/// per channel, independently for R, G, and B.
pub fn compute_scb_params(pixels: &[u8], channels: usize, clip_fraction: f64) -> ColorCorrectionParams {
    let mut black = [0u8; 3];
    let mut white = [0u8; 3];
    for (channel_index, (b, w)) in black.iter_mut().zip(white.iter_mut()).enumerate() {
        let histogram = channel_histogram(pixels, channels, channel_index);
        let (channel_black, channel_white) = clip_points(&histogram, clip_fraction);
        *b = channel_black;
        *w = channel_white;
    }
    ColorCorrectionParams { black, white }
}

/// The 256-entry input->output lookup table for one channel's stretch: `v <= black` maps to 0,
/// `v >= white` maps to 255, and values in between are linearly interpolated. Built once per
/// channel per render rather than recomputed per pixel.
pub fn build_lut(black: u8, white: u8) -> [u8; 256] {
    let mut lut = [0u8; 256];
    let (black, white) = (black as i32, white as i32);
    let span = (white - black).max(1) as f64;
    for (value, entry) in lut.iter_mut().enumerate() {
        let value = value as i32;
        *entry = if value <= black {
            0
        } else if value >= white {
            255
        } else {
            (((value - black) as f64 / span) * 255.0).round() as u8
        };
    }
    lut
}

/// Rebuilds `ColorCorrectionParams` from `ImageRecord`'s six flat nullable columns — `None` if
/// any of the six is `None` (matches `rotation`'s "never run yet" semantics; the six are always
/// written together by `image_viewer::toggle_auto_correct`, so a partial `None` shouldn't
/// occur in practice, but this stays defensive either way).
pub fn from_record(record: &ImageRecord) -> Option<ColorCorrectionParams> {
    Some(ColorCorrectionParams {
        black: [record.color_black_r?, record.color_black_g?, record.color_black_b?],
        white: [record.color_white_r?, record.color_white_g?, record.color_white_b?],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_histogram_counts_each_value_into_its_own_bucket() {
        // Three pixels' R channel (byte 0 of each, channels=3): 10, 10, 20.
        let pixels = [10u8, 0, 0, 10, 0, 0, 20, 0, 0];
        let histogram = channel_histogram(&pixels, 3, 0);
        assert_eq!(histogram[10], 2);
        assert_eq!(histogram[20], 1);
        assert_eq!(histogram.iter().sum::<u32>(), 3);
    }

    #[test]
    fn channel_histogram_only_reads_the_selected_channel_from_an_interleaved_buffer() {
        // RGBA: R=1,G=2,B=3,A=4 for every pixel.
        let pixels = [1u8, 2, 3, 4, 1, 2, 3, 4];
        assert_eq!(channel_histogram(&pixels, 4, 0)[1], 2);
        assert_eq!(channel_histogram(&pixels, 4, 1)[2], 2);
        assert_eq!(channel_histogram(&pixels, 4, 2)[3], 2);
    }

    #[test]
    fn clip_points_clips_the_requested_fraction_from_each_tail() {
        // 100 pixels: values 0..=99 once each. Clipping 1% from each tail should push the
        // black point past value 0 and the white point below value 99.
        let mut histogram = [0u32; 256];
        for value in 0..100u32 {
            histogram[value as usize] = 1;
        }
        let (black, white) = clip_points(&histogram, 0.01);
        assert_eq!(black, 1);
        assert_eq!(white, 98);
    }

    #[test]
    fn clip_points_falls_back_to_full_range_for_an_empty_histogram() {
        assert_eq!(clip_points(&[0u32; 256], 0.01), (0, 255));
    }

    #[test]
    fn clip_points_never_produces_black_greater_than_white() {
        // Every pixel is exactly the same value — both tails' clip thresholds land on it.
        let mut histogram = [0u32; 256];
        histogram[128] = 1000;
        assert_eq!(clip_points(&histogram, 0.01), (0, 255));
    }

    #[test]
    fn compute_scb_params_derives_each_channel_independently() {
        // R channel is all 100s, G all 150s, B all 200s — each channel's clip points should
        // collapse to that single value on both ends (no spread to clip within).
        let mut pixels = Vec::new();
        for _ in 0..10 {
            pixels.extend_from_slice(&[100u8, 150, 200]);
        }
        let params = compute_scb_params(&pixels, 3, 0.01);
        // A single-valued channel has no spread to clip — `clip_points` falls back to (0, 255).
        assert_eq!(params.black, [0, 0, 0]);
        assert_eq!(params.white, [255, 255, 255]);
    }

    #[test]
    fn build_lut_clips_below_black_and_above_white_and_interpolates_between() {
        let lut = build_lut(50, 200);
        assert_eq!(lut[0], 0);
        assert_eq!(lut[50], 0);
        assert_eq!(lut[200], 255);
        assert_eq!(lut[255], 255);
        // Midpoint between 50 and 200 (125) should map roughly to the middle of the output range.
        assert!(lut[125] > 100 && lut[125] < 155);
    }

    #[test]
    fn build_lut_is_identity_for_the_full_range() {
        let lut = build_lut(0, 255);
        for value in 0..=255u8 {
            assert_eq!(lut[value as usize], value);
        }
    }

    fn record_with_correction() -> ImageRecord {
        ImageRecord {
            color_black_r: Some(1),
            color_black_g: Some(2),
            color_black_b: Some(3),
            color_white_r: Some(250),
            color_white_g: Some(251),
            color_white_b: Some(252),
            ..ImageRecord::default()
        }
    }

    #[test]
    fn from_record_is_none_when_any_of_the_six_fields_is_null() {
        let mut record = record_with_correction();
        record.color_white_b = None;
        assert_eq!(from_record(&record), None);
        assert_eq!(from_record(&ImageRecord::default()), None);
    }

    #[test]
    fn from_record_reassembles_all_six_fields_when_present() {
        let params = from_record(&record_with_correction()).unwrap();
        assert_eq!(params.black, [1, 2, 3]);
        assert_eq!(params.white, [250, 251, 252]);
    }
}
