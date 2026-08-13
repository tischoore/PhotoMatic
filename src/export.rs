use std::mem;
use std::path::{Path, PathBuf};
use std::ptr;

use native_windows_gui as nwg;

use crate::color_correction::ColorCorrectionParams;
use crate::image_viewer;

/// The options collected by the Export dialog (`export_modal.rs`) and handed to the Exporting
/// dialog's worker thread (`export_progress_modal.rs`) to act on.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub recompression: Recompression,
    /// The requested resize target, or `None` for no resizing. Always `None` when
    /// `recompression` is `Recompression::None` — resizing implies re-encoding.
    pub rescale: Option<RescaleTarget>,
    pub output_dir: PathBuf,
}

/// A rescale request: at least one of `width`/`height` must be set (enforced by
/// `export_modal::validate`) — the image is always scaled to preserve its original aspect ratio,
/// so the other dimension, if omitted, is derived rather than requested directly. See
/// [`scaled_output_size`] for how a request with both set is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RescaleTarget {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// What happened over the course of one export run, reported by the Exporting dialog's worker
/// thread once it stops (whether by finishing or by Abort).
#[derive(Debug, Default)]
pub struct ExportOutcome {
    pub copied: usize,
    pub total: usize,
    pub aborted: bool,
    /// One message per picture that failed to copy/encode — collected rather than treated as
    /// fatal, so one bad file doesn't stop the rest of the collection from exporting.
    pub errors: Vec<String>,
}

/// A human-readable summary of `outcome`, shown after an export that was aborted or hit errors
/// (a fully clean, non-aborted export stays silent, matching `App::scan_finished`'s lack of a
/// success popup).
pub fn summary_message(outcome: &ExportOutcome) -> String {
    let mut message = if outcome.aborted {
        format!("Export aborted after {} of {} pictures.", outcome.copied, outcome.total)
    } else {
        format!("Exported {} of {} pictures.", outcome.copied, outcome.total)
    };
    if !outcome.errors.is_empty() {
        message.push_str("\n\nThe following pictures failed:\n");
        message.push_str(&outcome.errors.join("\n"));
    }
    message
}

/// The three export recompression choices: a byte-identical copy, or a JPEG re-encode at one of
/// two fixed quality levels. `Large`/`Small` are the only variants that trigger the WIC encode
/// pipeline in [`export_one`] — `None` is always a plain `std::fs::copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Recompression {
    #[default]
    None,
    Large,
    Small,
}

impl Recompression {
    /// The JPEG quality (0.0-1.0) to encode at, or `None` for `Recompression::None` (no
    /// re-encoding happens at all in that case).
    pub fn jpeg_quality(self) -> Option<f32> {
        match self {
            Recompression::None => None,
            Recompression::Large => Some(0.8),
            Recompression::Small => Some(0.5),
        }
    }
}

/// How many digits are needed to print `picture_count` itself — the width every exported
/// filename's index prefix is zero-padded to, so e.g. a 125-picture collection numbers files
/// `001_`..`125_`.
pub fn digit_count(picture_count: usize) -> usize {
    picture_count.to_string().len()
}

/// The destination filename for the `index_1_based`-th picture (1-based) in a collection:
/// `original_file_name` prefixed with its zero-padded index. Recompressing (`Large`/`Small`)
/// swaps the extension to `jpg` (the file is being re-encoded as JPEG regardless of its source
/// format); `Recompression::None` keeps the original extension, since the file is copied as-is.
pub fn indexed_file_name(
    index_1_based: usize,
    digit_count: usize,
    original_file_name: &str,
    recompression: Recompression,
) -> String {
    let stem = Path::new(original_file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(original_file_name);
    let extension = match recompression {
        Recompression::None => Path::new(original_file_name).extension().and_then(|s| s.to_str()),
        Recompression::Large | Recompression::Small => Some("jpg"),
    };
    let prefix = format!("{index_1_based:0digit_count$}");
    match extension {
        Some(extension) if !extension.is_empty() => format!("{prefix}_{stem}.{extension}"),
        _ => format!("{prefix}_{stem}"),
    }
}

/// The aspect-ratio-preserving output size for `target` against `source_size` (the image's
/// actual size in its final, post-rotation orientation — i.e. what the user perceives as "the
/// photo"'s width/height). Exactly one of `target.width`/`target.height` drives the scale
/// directly; the other dimension is derived to keep `source_size`'s aspect ratio. When both are
/// given, whichever one is numerically *closer* to `source_size`'s matching dimension drives the
/// scale — measured as plain pixel distance, not a ratio — so the image is resized as little as
/// possible to satisfy the request.
pub fn scaled_output_size(source_size: (u32, u32), target: RescaleTarget) -> (u32, u32) {
    let (source_width, source_height) = source_size;

    let from_width = |width: u32| -> (u32, u32) {
        let height = ((source_height as f64) * (width as f64) / (source_width as f64)).round().max(1.0);
        (width, height as u32)
    };
    let from_height = |height: u32| -> (u32, u32) {
        let width = ((source_width as f64) * (height as f64) / (source_height as f64)).round().max(1.0);
        (width as u32, height)
    };

    match (target.width, target.height) {
        (Some(width), None) => from_width(width),
        (None, Some(height)) => from_height(height),
        (None, None) => source_size,
        (Some(width), Some(height)) => {
            let width_distance = (source_width as i64 - width as i64).abs();
            let height_distance = (source_height as i64 - height as i64).abs();
            if width_distance <= height_distance {
                from_width(width)
            } else {
                from_height(height)
            }
        }
    }
}

/// Whether `path` exists and already contains at least one entry — used to warn before exporting
/// into a directory that isn't empty. A path that doesn't exist (or can't be read) is treated as
/// "not non-empty": `export_progress_modal`'s worker thread creates it via `create_dir_all`
/// before writing anything.
pub fn directory_is_non_empty(path: &Path) -> bool {
    std::fs::read_dir(path).map(|mut entries| entries.next().is_some()).unwrap_or(false)
}

/// Exports a single picture from `src` to `dest`.
///
/// `Recompression::None` is a plain, byte-identical `std::fs::copy` — no WIC/COM involved, no
/// rotation or color correction applied. `Large`/`Small` decode the full-resolution source,
/// optionally resize it to `rescale` (aspect-ratio-preserving — see `scaled_output_size` —
/// computed against the image's *final*, post-rotation orientation, then converted back to a
/// pre-rotation resize target via a width/height swap when `rotation_degrees` is 90/270, exactly
/// as `image_viewer::decode_and_fit` already does for on-screen display, since resizing happens
/// before rotating), bake in `color_params` (if the photo has ever had Auto Correct run on it)
/// and `rotation_degrees` via the same WIC helpers `image_viewer` uses for display — in that
/// order, mirroring `decode_and_fit`'s own "materialize -> correct -> rotate" sequence — and
/// re-encode as JPEG at the chosen quality. This is the one part of the export feature with no
/// prior WIC-encoder precedent in this codebase (decode/scale/correct/rotate all reuse
/// `image_viewer`'s helpers; encoding is new) — it has no unit tests, matching this codebase's
/// existing practice of leaving WIC COM calls (`image_viewer::rotate_frame`/`materialize_bitmap`/
/// `apply_color_correction`) untested and verified manually instead.
pub fn export_one(
    src: &Path,
    dest: &Path,
    rotation_degrees: i32,
    color_params: Option<ColorCorrectionParams>,
    recompression: Recompression,
    rescale: Option<RescaleTarget>,
) -> Result<(), String> {
    let Some(quality) = recompression.jpeg_quality() else {
        return std::fs::copy(src, dest).map(|_| ()).map_err(|e| e.to_string());
    };

    let decoder = nwg::ImageDecoder::new().map_err(|e| e.to_string())?;
    let source = decoder.from_filename(&src.to_string_lossy()).map_err(|e| e.to_string())?;
    let frame = source.frame(0).map_err(|e| e.to_string())?;

    let scaled = match rescale {
        Some(target) => {
            let post_rotation_size = image_viewer::swapped_for_rotation(frame.size(), rotation_degrees);
            let output_size = scaled_output_size(post_rotation_size, target);
            // Resizing happens before rotating, so the target passed to `resize_image` is the
            // pre-rotation size that will *become* `output_size` once rotated — the same swap
            // used to get from `frame.size()` to `post_rotation_size` above, applied in reverse
            // (`swapped_for_rotation` is its own inverse).
            let pre_rotation_target = image_viewer::swapped_for_rotation(output_size, rotation_degrees);
            decoder
                .resize_image(&frame, [pre_rotation_target.0, pre_rotation_target.1])
                .map_err(|e| e.to_string())?
        }
        None => frame,
    };

    let materialized = image_viewer::materialize_bitmap(&decoder, &scaled)
        .ok_or_else(|| "Failed to prepare the image for rotation".to_string())?;
    let corrected = match color_params {
        Some(params) => image_viewer::apply_color_correction(&decoder, &materialized, &params)
            .ok_or_else(|| "Failed to apply color correction".to_string())?,
        None => materialized,
    };
    let rotated = image_viewer::rotate_frame(&decoder, &corrected, rotation_degrees)
        .ok_or_else(|| "Failed to apply rotation".to_string())?;

    write_jpeg(&decoder, &rotated, dest, quality)
}

/// Encodes `image` as a JPEG file at `dest` via WIC's `IWICBitmapEncoder`, at `quality` (0.0-1.0).
/// No prior usage of `IWICBitmapEncoder`/`IWICBitmapFrameEncode`/`IPropertyBag2` exists elsewhere
/// in this codebase (`image_viewer.rs` only ever decodes/scales/rotates for on-screen display) —
/// this is new COM interop, verified manually rather than with a unit test.
fn write_jpeg(decoder: &nwg::ImageDecoder, image: &nwg::ImageData, dest: &Path, quality: f32) -> Result<(), String> {
    use winapi::shared::winerror::S_OK;
    use winapi::shared::wtypes::VT_R4;
    use winapi::um::oaidl::VARIANT;
    use winapi::um::objidlbase::IStream;
    use winapi::um::ocidl::{IPropertyBag2, PROPBAG2};
    use winapi::um::wincodec::{
        GUID_ContainerFormatJpeg, IWICBitmapEncoder, IWICBitmapFrameEncode, IWICBitmapSource,
        IWICImagingFactory, IWICStream, WICBitmapEncoderNoCache,
    };
    use winapi::um::winnt::GENERIC_WRITE;

    let (width, height) = image.size();
    let wide_dest = to_wide_null(&dest.to_string_lossy());
    let mut quality_property_name = to_wide_null("ImageQuality");

    unsafe {
        let factory = &*(decoder.factory as *const IWICImagingFactory);

        let mut stream: *mut IWICStream = ptr::null_mut();
        if factory.CreateStream(&mut stream) != S_OK {
            return Err("Failed to create an output stream".to_string());
        }
        if (&*stream).InitializeFromFilename(wide_dest.as_ptr(), GENERIC_WRITE) != S_OK {
            return Err(format!("Failed to open {} for writing", dest.display()));
        }

        let mut encoder: *mut IWICBitmapEncoder = ptr::null_mut();
        if factory.CreateEncoder(&GUID_ContainerFormatJpeg, ptr::null(), &mut encoder) != S_OK {
            return Err("Failed to create a JPEG encoder".to_string());
        }
        if (&*encoder).Initialize(stream as *const IStream, WICBitmapEncoderNoCache) != S_OK {
            return Err("Failed to initialize the JPEG encoder".to_string());
        }

        let mut frame_encode: *mut IWICBitmapFrameEncode = ptr::null_mut();
        let mut property_bag: *mut IPropertyBag2 = ptr::null_mut();
        if (&*encoder).CreateNewFrame(&mut frame_encode, &mut property_bag) != S_OK {
            return Err("Failed to create a JPEG frame".to_string());
        }

        let mut option: PROPBAG2 = mem::zeroed();
        option.pstrName = quality_property_name.as_mut_ptr();

        let mut variant: VARIANT = mem::zeroed();
        let tag = variant.n1.n2_mut();
        tag.vt = VT_R4 as _;
        *tag.n3.fltVal_mut() = quality;

        if (&*property_bag).Write(1, &option, &variant) != S_OK {
            return Err("Failed to set the JPEG quality".to_string());
        }

        if (&*frame_encode).Initialize(property_bag) != S_OK {
            return Err("Failed to initialize the JPEG frame".to_string());
        }
        if (&*frame_encode).SetSize(width, height) != S_OK {
            return Err("Failed to set the JPEG frame size".to_string());
        }
        let source = image.frame as *const IWICBitmapSource;
        if (&*frame_encode).WriteSource(source, ptr::null()) != S_OK {
            return Err("Failed to write the image data".to_string());
        }
        if (&*frame_encode).Commit() != S_OK {
            return Err("Failed to finish the JPEG frame".to_string());
        }
        if (&*encoder).Commit() != S_OK {
            return Err(format!("Failed to finish writing {}", dest.display()));
        }

        (&*frame_encode).Release();
        (&*property_bag).Release();
        (&*encoder).Release();
        (&*stream).Release();
    }

    Ok(())
}

fn to_wide_null(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_message_reports_a_clean_completion() {
        let outcome = ExportOutcome { copied: 5, total: 5, aborted: false, errors: vec![] };
        assert_eq!(summary_message(&outcome), "Exported 5 of 5 pictures.");
    }

    #[test]
    fn summary_message_reports_an_abort() {
        let outcome = ExportOutcome { copied: 2, total: 5, aborted: true, errors: vec![] };
        assert_eq!(summary_message(&outcome), "Export aborted after 2 of 5 pictures.");
    }

    #[test]
    fn summary_message_lists_errors() {
        let outcome = ExportOutcome {
            copied: 4,
            total: 5,
            aborted: false,
            errors: vec!["beach.jpg: disk full".to_string()],
        };
        let message = summary_message(&outcome);
        assert!(message.contains("Exported 4 of 5 pictures."));
        assert!(message.contains("beach.jpg: disk full"));
    }

    #[test]
    fn digit_count_matches_the_digits_needed_to_print_the_count_itself() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(1), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(99), 2);
        assert_eq!(digit_count(100), 3);
        assert_eq!(digit_count(125), 3);
        assert_eq!(digit_count(999), 3);
        assert_eq!(digit_count(1000), 4);
    }

    #[test]
    fn indexed_file_name_zero_pads_the_index_to_the_requested_width() {
        assert_eq!(indexed_file_name(1, 3, "beach.jpg", Recompression::None), "001_beach.jpg");
        assert_eq!(indexed_file_name(42, 3, "beach.jpg", Recompression::None), "042_beach.jpg");
        assert_eq!(indexed_file_name(125, 3, "beach.jpg", Recompression::None), "125_beach.jpg");
    }

    #[test]
    fn indexed_file_name_keeps_the_original_extension_when_not_recompressing() {
        assert_eq!(indexed_file_name(1, 2, "photo.CR2", Recompression::None), "01_photo.CR2");
        assert_eq!(indexed_file_name(1, 2, "photo.png", Recompression::None), "01_photo.png");
    }

    #[test]
    fn indexed_file_name_swaps_the_extension_to_jpg_when_recompressing() {
        assert_eq!(indexed_file_name(1, 2, "photo.CR2", Recompression::Large), "01_photo.jpg");
        assert_eq!(indexed_file_name(1, 2, "photo.png", Recompression::Small), "01_photo.jpg");
    }

    #[test]
    fn indexed_file_name_preserves_the_original_stem_unchanged() {
        assert_eq!(indexed_file_name(1, 1, "my.trip.photo.jpg", Recompression::None), "1_my.trip.photo.jpg");
    }

    #[test]
    fn jpeg_quality_maps_each_recompression_choice() {
        assert_eq!(Recompression::None.jpeg_quality(), None);
        assert_eq!(Recompression::Large.jpeg_quality(), Some(0.8));
        assert_eq!(Recompression::Small.jpeg_quality(), Some(0.5));
    }

    #[test]
    fn scaled_output_size_scales_from_width_alone() {
        let target = RescaleTarget { width: Some(400), height: None };
        assert_eq!(scaled_output_size((1000, 500), target), (400, 200));
    }

    #[test]
    fn scaled_output_size_scales_from_height_alone() {
        let target = RescaleTarget { width: None, height: Some(100) };
        assert_eq!(scaled_output_size((1000, 500), target), (200, 100));
    }

    #[test]
    fn scaled_output_size_with_both_uses_whichever_dimension_is_closer_to_the_source() {
        // Source is 1000x500. Width 900 is 100px away; height 100 is 400px away — width wins.
        let target = RescaleTarget { width: Some(900), height: Some(100) };
        assert_eq!(scaled_output_size((1000, 500), target), (900, 450));

        // Now height 450 (50px away) is closer than width 100 (900px away) — height wins.
        let target = RescaleTarget { width: Some(100), height: Some(450) };
        assert_eq!(scaled_output_size((1000, 500), target), (900, 450));
    }

    #[test]
    fn scaled_output_size_ties_break_toward_width() {
        // Both 100px away from the 1000x500 source.
        let target = RescaleTarget { width: Some(1100), height: Some(400) };
        assert_eq!(scaled_output_size((1000, 500), target), (1100, 550));
    }

    #[test]
    fn scaled_output_size_returns_the_source_unchanged_when_neither_dimension_is_given() {
        let target = RescaleTarget { width: None, height: None };
        assert_eq!(scaled_output_size((1000, 500), target), (1000, 500));
    }

    #[test]
    fn directory_is_non_empty_is_false_for_a_missing_path() {
        assert!(!directory_is_non_empty(Path::new("Z:\\this\\path\\does\\not\\exist\\at\\all")));
    }

    #[test]
    fn directory_is_non_empty_reflects_directory_contents() {
        let dir = std::env::temp_dir().join(format!("photomatic_export_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!directory_is_non_empty(&dir));

        std::fs::write(dir.join("file.txt"), b"x").unwrap();
        assert!(directory_is_non_empty(&dir));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
