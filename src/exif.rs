use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use chrono::NaiveDateTime;
use exif::{Field, In, Tag, Value};

/// Photographic metadata read from an image's EXIF data. Every field is
/// `None` when the tag is absent or the file has no EXIF segment at all —
/// there is no error case, only "found" or "not found".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageMetadata {
    pub date_taken: Option<NaiveDateTime>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub exposure_time_seconds: Option<f64>,
    pub iso: Option<u32>,
    pub focal_length_mm: Option<f64>,
    /// Decimal degrees (WGS84), signed — north/east positive. `None` unless both the
    /// coordinate and its hemisphere ref tag are present and well-formed.
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    /// Meters, signed — below sea level negative.
    pub gps_altitude_m: Option<f64>,
}

/// Reads EXIF metadata from the image at `path`. Any failure to open the
/// file or find an EXIF segment (missing file, no EXIF, corrupt data)
/// results in an all-`None` `ImageMetadata` rather than an error — mirrors
/// `scan.rs`'s "unreadable → silently skipped" behavior, since a file
/// without usable metadata is not exceptional here.
pub fn read_metadata(path: &Path) -> ImageMetadata {
    let Ok(file) = File::open(path) else {
        return ImageMetadata::default();
    };
    let mut reader = BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return ImageMetadata::default();
    };

    let date_taken = field(&exif, Tag::DateTimeOriginal)
        .or_else(|| field(&exif, Tag::DateTime))
        .and_then(ascii_string)
        .and_then(|s| parse_exif_datetime(&s));

    let width = field(&exif, Tag::PixelXDimension)
        .and_then(uint_value)
        .or_else(|| field(&exif, Tag::ImageWidth).and_then(uint_value));
    let height = field(&exif, Tag::PixelYDimension)
        .and_then(uint_value)
        .or_else(|| field(&exif, Tag::ImageLength).and_then(uint_value));

    let exposure_time_seconds = field(&exif, Tag::ExposureTime).and_then(rational_f64);
    let iso = field(&exif, Tag::PhotographicSensitivity).and_then(uint_value);
    let focal_length_mm = field(&exif, Tag::FocalLength).and_then(rational_f64);

    let gps_latitude = gps_coordinate(&exif, Tag::GPSLatitude, Tag::GPSLatitudeRef);
    let gps_longitude = gps_coordinate(&exif, Tag::GPSLongitude, Tag::GPSLongitudeRef);
    let gps_altitude_m = gps_altitude(&exif);

    ImageMetadata {
        date_taken,
        width,
        height,
        exposure_time_seconds,
        iso,
        focal_length_mm,
        gps_latitude,
        gps_longitude,
        gps_altitude_m,
    }
}

fn field<'a>(exif: &'a exif::Exif, tag: Tag) -> Option<&'a Field> {
    exif.get_field(tag, In::PRIMARY)
}

fn ascii_string(field: &Field) -> Option<String> {
    match &field.value {
        Value::Ascii(strings) => {
            strings.first().map(|bytes| String::from_utf8_lossy(bytes).trim().to_string())
        }
        _ => None,
    }
}

fn uint_value(field: &Field) -> Option<u32> {
    field.value.get_uint(0)
}

fn rational_f64(field: &Field) -> Option<f64> {
    match &field.value {
        Value::Rational(values) => values.first().map(|r| r.to_f64()),
        _ => None,
    }
}

/// Parses EXIF's fixed `"YYYY:MM:DD HH:MM:SS"` datetime format. Returns
/// `None` for anything else, rather than erroring — a malformed or
/// non-conforming string is treated the same as a missing tag.
fn parse_exif_datetime(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y:%m:%d %H:%M:%S").ok()
}

/// Converts a degrees/minutes/seconds coordinate to signed decimal degrees. `hemisphere`
/// is the EXIF ref string (`"N"`/`"S"` for latitude, `"E"`/`"W"` for longitude); `"S"` and
/// `"W"` negate the magnitude, anything else leaves it positive.
fn dms_to_decimal_degrees(degrees: f64, minutes: f64, seconds: f64, hemisphere: &str) -> f64 {
    let magnitude = degrees + minutes / 60.0 + seconds / 3600.0;
    match hemisphere {
        "S" | "W" => -magnitude,
        _ => magnitude,
    }
}

/// Reads a GPS latitude or longitude: `value_tag` holds 3 rationals (degrees, minutes,
/// seconds), `ref_tag` holds the hemisphere. `None` if either tag is missing or not in
/// the expected shape — a coordinate without a hemisphere ref is unusable.
fn gps_coordinate(exif: &exif::Exif, value_tag: Tag, ref_tag: Tag) -> Option<f64> {
    let dms = match &field(exif, value_tag)?.value {
        Value::Rational(values) if values.len() == 3 => {
            [values[0].to_f64(), values[1].to_f64(), values[2].to_f64()]
        }
        _ => return None,
    };
    let hemisphere = field(exif, ref_tag).and_then(ascii_string)?;
    Some(dms_to_decimal_degrees(dms[0], dms[1], dms[2], &hemisphere))
}

/// Negates `meters` when `below_sea_level` (EXIF's `GPSAltitudeRef` byte `1`); a
/// missing/malformed ref defaults to above sea level per the EXIF spec.
fn apply_altitude_ref(meters: f64, below_sea_level: bool) -> f64 {
    if below_sea_level {
        -meters
    } else {
        meters
    }
}

fn gps_altitude(exif: &exif::Exif) -> Option<f64> {
    let meters = field(exif, Tag::GPSAltitude).and_then(rational_f64)?;
    let below_sea_level = field(exif, Tag::GPSAltitudeRef).and_then(uint_value) == Some(1);
    Some(apply_altitude_ref(meters, below_sea_level))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn fixture_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_exif.jpg")
    }

    #[test]
    fn parse_exif_datetime_parses_the_standard_exif_format() {
        let parsed = parse_exif_datetime("2024:06:15 12:30:45").unwrap();
        assert_eq!(parsed, NaiveDate::from_ymd_opt(2024, 6, 15).unwrap().and_hms_opt(12, 30, 45).unwrap());
    }

    #[test]
    fn parse_exif_datetime_returns_none_for_malformed_input() {
        assert_eq!(parse_exif_datetime("not a date"), None);
        assert_eq!(parse_exif_datetime(""), None);
        assert_eq!(parse_exif_datetime("2024-06-15 12:30:45"), None);
    }

    #[test]
    fn read_metadata_extracts_all_known_tags_from_a_real_jpeg() {
        let metadata = read_metadata(&fixture_path());

        assert_eq!(
            metadata.date_taken,
            Some(NaiveDate::from_ymd_opt(2024, 6, 15).unwrap().and_hms_opt(12, 30, 45).unwrap())
        );
        assert_eq!(metadata.width, Some(800));
        assert_eq!(metadata.height, Some(600));
        assert_eq!(metadata.exposure_time_seconds, Some(1.0 / 500.0));
        assert_eq!(metadata.iso, Some(200));
        assert_eq!(metadata.focal_length_mm, Some(50.0));
        assert_eq!(metadata.gps_latitude, None);
        assert_eq!(metadata.gps_longitude, None);
        assert_eq!(metadata.gps_altitude_m, None);
    }

    #[test]
    fn read_metadata_returns_all_none_when_the_file_has_no_exif_segment() {
        let dir = std::env::temp_dir().join(format!("photomatic-test-exif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("no_exif.gif");
        std::fs::write(&path, b"GIF89a").unwrap();

        assert_eq!(read_metadata(&path), ImageMetadata::default());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_metadata_does_not_panic_on_a_missing_or_corrupt_file() {
        let missing = Path::new("this/path/does/not/exist.jpg");
        assert_eq!(read_metadata(missing), ImageMetadata::default());

        let dir = std::env::temp_dir().join(format!("photomatic-test-exif-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("corrupt.jpg");
        std::fs::write(&path, b"\xff\xd8not actually a valid jpeg").unwrap();

        assert_eq!(read_metadata(&path), ImageMetadata::default());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dms_to_decimal_degrees_stays_positive_for_north_and_east() {
        assert_eq!(dms_to_decimal_degrees(40.0, 26.0, 46.8, "N"), 40.0 + 26.0 / 60.0 + 46.8 / 3600.0);
        assert_eq!(dms_to_decimal_degrees(1.0, 0.0, 0.0, "E"), 1.0);
    }

    #[test]
    fn dms_to_decimal_degrees_negates_for_south_and_west() {
        assert_eq!(dms_to_decimal_degrees(79.0, 58.0, 55.2, "W"), -(79.0 + 58.0 / 60.0 + 55.2 / 3600.0));
        assert_eq!(dms_to_decimal_degrees(1.0, 0.0, 0.0, "S"), -1.0);
    }

    #[test]
    fn apply_altitude_ref_negates_only_when_below_sea_level() {
        assert_eq!(apply_altitude_ref(100.0, false), 100.0);
        assert_eq!(apply_altitude_ref(100.0, true), -100.0);
    }

    /// Builds a minimal JPEG containing only a GPS IFD (latitude, longitude, altitude —
    /// no date/dimension/exposure tags), byte-for-byte, the same way the tiny
    /// `sample_exif.jpg` fixture was hand-built. There's no EXIF-writing tool in this
    /// environment to regenerate a checked-in fixture, so the bytes are assembled here
    /// instead, with each section's length asserted against the next section's offset.
    fn build_gps_jpeg() -> Vec<u8> {
        fn le16(v: u16) -> [u8; 2] {
            v.to_le_bytes()
        }
        fn le32(v: u32) -> [u8; 4] {
            v.to_le_bytes()
        }
        fn tiff_entry(tag: u16, typ: u16, count: u32, value: [u8; 4]) -> [u8; 12] {
            let mut e = [0u8; 12];
            e[0..2].copy_from_slice(&le16(tag));
            e[2..4].copy_from_slice(&le16(typ));
            e[4..8].copy_from_slice(&le32(count));
            e[8..12].copy_from_slice(&value);
            e
        }
        fn rational(num: u32, den: u32) -> [u8; 8] {
            let mut b = [0u8; 8];
            b[0..4].copy_from_slice(&le32(num));
            b[4..8].copy_from_slice(&le32(den));
            b
        }
        fn ascii_inline(s: &[u8]) -> [u8; 4] {
            let mut v = [0u8; 4];
            v[..s.len()].copy_from_slice(s);
            v
        }

        // TIFF layout (offsets relative to the TIFF header):
        //   0   "II*\0" + IFD0 offset (8 bytes)
        //   8   IFD0: 1 entry (GPS IFD pointer) + next-IFD offset (18 bytes) -> ends at 26
        //   26  GPS IFD: 6 entries + next-IFD offset (78 bytes)              -> ends at 104
        //   104 latitude DMS rationals (24 bytes)                           -> ends at 128
        //   128 longitude DMS rationals (24 bytes)                         -> ends at 152
        //   152 altitude rational (8 bytes)                                -> ends at 160
        let gps_ifd_offset: u32 = 26;
        let lat_data_offset: u32 = 104;
        let lon_data_offset: u32 = 128;
        let alt_data_offset: u32 = 152;

        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&le32(8));

        tiff.extend_from_slice(&le16(1));
        tiff.extend_from_slice(&tiff_entry(0x8825, 4, 1, le32(gps_ifd_offset))); // GPSInfoIFDPointer
        tiff.extend_from_slice(&le32(0));
        assert_eq!(tiff.len() as u32, gps_ifd_offset);

        tiff.extend_from_slice(&le16(6));
        tiff.extend_from_slice(&tiff_entry(0x0001, 2, 2, ascii_inline(b"N\0"))); // GPSLatitudeRef
        tiff.extend_from_slice(&tiff_entry(0x0002, 5, 3, le32(lat_data_offset))); // GPSLatitude
        tiff.extend_from_slice(&tiff_entry(0x0003, 2, 2, ascii_inline(b"W\0"))); // GPSLongitudeRef
        tiff.extend_from_slice(&tiff_entry(0x0004, 5, 3, le32(lon_data_offset))); // GPSLongitude
        tiff.extend_from_slice(&tiff_entry(0x0005, 1, 1, [0, 0, 0, 0])); // GPSAltitudeRef: above sea level
        tiff.extend_from_slice(&tiff_entry(0x0006, 5, 1, le32(alt_data_offset))); // GPSAltitude
        tiff.extend_from_slice(&le32(0));
        assert_eq!(tiff.len() as u32, lat_data_offset);

        tiff.extend_from_slice(&rational(40, 1)); // 40 deg
        tiff.extend_from_slice(&rational(26, 1)); // 26 min
        tiff.extend_from_slice(&rational(468, 10)); // 46.8 sec
        assert_eq!(tiff.len() as u32, lon_data_offset);

        tiff.extend_from_slice(&rational(79, 1)); // 79 deg
        tiff.extend_from_slice(&rational(58, 1)); // 58 min
        tiff.extend_from_slice(&rational(552, 10)); // 55.2 sec
        assert_eq!(tiff.len() as u32, alt_data_offset);

        tiff.extend_from_slice(&rational(1234, 10)); // 123.4 m

        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&[0xff, 0xd8]); // SOI
        jpeg.extend_from_slice(&[0xff, 0xe1]); // APP1
        jpeg.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes()); // segment length (big-endian)
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&[0xff, 0xd9]); // EOI
        jpeg
    }

    #[test]
    fn read_metadata_extracts_gps_coordinates_from_a_real_jpeg() {
        let dir = std::env::temp_dir().join(format!("photomatic-test-exif-gps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gps.jpg");
        std::fs::write(&path, build_gps_jpeg()).unwrap();

        let metadata = read_metadata(&path);

        let expected_lat = 40.0 + 26.0 / 60.0 + 46.8 / 3600.0;
        let expected_lon = -(79.0 + 58.0 / 60.0 + 55.2 / 3600.0);
        assert_eq!(metadata.gps_latitude, Some(expected_lat));
        assert_eq!(metadata.gps_longitude, Some(expected_lon));
        assert_eq!(metadata.gps_altitude_m, Some(123.4));

        std::fs::remove_dir_all(&dir).ok();
    }
}
