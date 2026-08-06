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

    ImageMetadata { date_taken, width, height, exposure_time_seconds, iso, focal_length_mm }
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
}
