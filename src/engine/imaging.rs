use crate::config::{ImageSizingMode, ResizeMethod};
use image::DynamicImage;

/// Everything that decides how a decoded page is scaled before display.
#[derive(Clone, Copy)]
pub struct ImageProcessingSpec {
    pub resize_method: ResizeMethod,
    pub image_sizing_mode: ImageSizingMode,
    pub transparency_support: bool,
    pub zoom_factor: f32,
    /// ルーペを出しているとき。拡大して見るので、ズーム中と同じく高い解像度で読み込む
    pub loupe: bool,
    /// Size of the area the page is drawn into, in points.
    pub container_size: [f32; 2],
}

/// Pixel size a `width` x `height` image is resized to, or `None` when it is
/// shown as is. While zoomed (or while the loupe is shown) the target is 3x
/// larger so the texture stays sharp.
pub fn display_size(width: u32, height: u32, spec: &ImageProcessingSpec) -> Option<(u32, u32)> {
    spec.resize_method.to_filter()?;
    if spec.image_sizing_mode == ImageSizingMode::OriginalSize {
        return None;
    }
    let [container_w, container_h] = spec.container_size;
    let aspect_ratio = width as f32 / height as f32;
    let factor = if (spec.zoom_factor - 1.0).abs() > 0.01 || spec.loupe {
        3
    } else {
        1
    };
    let (target_w, target_h) = match spec.image_sizing_mode {
        ImageSizingMode::FitHeight => {
            let target_h = container_h.max(1.0) as u32;
            let target_w = (target_h as f32 * aspect_ratio) as u32;
            (target_w, target_h)
        }
        ImageSizingMode::FitWidth => {
            let target_w = container_w.max(1.0) as u32;
            let target_h = (target_w as f32 / aspect_ratio) as u32;
            (target_w, target_h)
        }
        ImageSizingMode::FitBoth => {
            let width_scale = container_w.max(1.0) / width as f32;
            let height_scale = container_h.max(1.0) / height as f32;
            let scale = width_scale.min(height_scale);
            let target_w = (width as f32 * scale) as u32;
            let target_h = (height as f32 * scale) as u32;
            (target_w, target_h)
        }
        ImageSizingMode::OriginalSize => unreachable!(),
    };
    Some((target_w.max(1) * factor, target_h.max(1) * factor))
}

pub fn resize_for_display(img: DynamicImage, spec: &ImageProcessingSpec) -> DynamicImage {
    match (
        spec.resize_method.to_filter(),
        display_size(img.width(), img.height(), spec),
    ) {
        (Some(filter_type), Some((target_w, target_h))) => {
            img.resize(target_w, target_h, filter_type)
        }
        _ => img,
    }
}

pub fn strip_adobe_app14_if_invalid(bytes: &[u8]) -> Vec<u8> {
    let mut i = 2;
    let mut out = Vec::with_capacity(bytes.len());

    // Copy SOI first (must be first two bytes)
    if bytes.len() < 2 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return bytes.to_vec(); // not a valid jpeg
    }

    out.extend_from_slice(&bytes[0..2]);

    while i < bytes.len() {
        if i + 1 >= bytes.len() {
            break;
        }

        // Every marker must start with FF
        if bytes[i] != 0xFF {
            // Start of entropy data (after SOS)
            out.extend_from_slice(&bytes[i..]);
            break;
        }

        let marker = bytes[i + 1];

        // Standalone markers (no length)
        if marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            out.push(0xFF);
            out.push(marker);
            i += 2;
            continue;
        }

        // SOS marker copy rest of file and stop parsing
        if marker == 0xDA {
            out.extend_from_slice(&bytes[i..]);
            break;
        }

        if i + 4 > bytes.len() {
            break;
        }

        let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        let segment_end = i + 2 + length;

        if segment_end > bytes.len() {
            break;
        }

        // If APP14 (FF EE)
        if marker == 0xEE && length >= 14 && &bytes[i + 4..i + 9] == b"Adobe" {
            println!("Stripping Adobe APP14 segment");
            // Skip this segment entirely
            i = segment_end;
            continue;
        }

        // Otherwise copy full segment
        out.extend_from_slice(&bytes[i..segment_end]);
        i = segment_end;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(mode: ImageSizingMode, container: [f32; 2], zoom: f32) -> ImageProcessingSpec {
        ImageProcessingSpec {
            resize_method: ResizeMethod::Triangle,
            image_sizing_mode: mode,
            transparency_support: false,
            zoom_factor: zoom,
            loupe: false,
            container_size: container,
        }
    }

    #[test]
    fn fit_height_keeps_aspect_ratio() {
        let s = spec(ImageSizingMode::FitHeight, [800.0, 1000.0], 1.0);
        assert_eq!(display_size(1000, 2000, &s), Some((500, 1000)));
    }

    #[test]
    fn fit_width_keeps_aspect_ratio() {
        let s = spec(ImageSizingMode::FitWidth, [800.0, 1000.0], 1.0);
        assert_eq!(display_size(1000, 2000, &s), Some((800, 1600)));
    }

    #[test]
    fn fit_both_uses_the_tighter_side() {
        let s = spec(ImageSizingMode::FitBoth, [800.0, 1000.0], 1.0);
        assert_eq!(display_size(1000, 2000, &s), Some((500, 1000)));
        assert_eq!(display_size(2000, 1000, &s), Some((800, 400)));
    }

    #[test]
    fn zoom_renders_at_triple_resolution() {
        let s = spec(ImageSizingMode::FitHeight, [800.0, 1000.0], 1.5);
        assert_eq!(display_size(1000, 2000, &s), Some((1500, 3000)));
    }

    #[test]
    fn loupe_renders_at_triple_resolution() {
        let s = ImageProcessingSpec {
            loupe: true,
            ..spec(ImageSizingMode::FitHeight, [800.0, 1000.0], 1.0)
        };
        assert_eq!(display_size(1000, 2000, &s), Some((1500, 3000)));
    }

    #[test]
    fn original_size_and_no_filter_skip_resizing() {
        let s = spec(ImageSizingMode::OriginalSize, [800.0, 1000.0], 1.0);
        assert_eq!(display_size(1000, 2000, &s), None);
        let s = ImageProcessingSpec {
            resize_method: ResizeMethod::None,
            ..spec(ImageSizingMode::FitHeight, [800.0, 1000.0], 1.0)
        };
        assert_eq!(display_size(1000, 2000, &s), None);
        let img = DynamicImage::new_rgb8(1000, 2000);
        assert_eq!(resize_for_display(img, &s).height(), 2000);
    }

    #[test]
    fn strips_adobe_app14_segment() {
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(&[0xFF, 0xEE, 0x00, 0x0E]);
        jpeg.extend_from_slice(b"Adobe\0\0\0\0\0\0\0");
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x01, 0x02, 0xFF, 0xD9]);
        assert_eq!(
            strip_adobe_app14_if_invalid(&jpeg),
            vec![0xFF, 0xD8, 0xFF, 0xDA, 0x01, 0x02, 0xFF, 0xD9]
        );
    }

    #[test]
    fn non_jpeg_bytes_are_untouched() {
        assert_eq!(strip_adobe_app14_if_invalid(b"PNG"), b"PNG".to_vec());
    }
}
