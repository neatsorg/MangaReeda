use super::{ANIMATION_SCAN_AHEAD_PAGES, MangaReader};
use crate::config::{PageViewOptions, SourceMode};
use crate::engine::imaging::ImageProcessingSpec;
use crate::engine::source::read_entry;
use eframe::egui;
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage, ImageFormat};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(super) struct AnimatedFrame {
    pub(super) image: egui::ColorImage,
    pub(super) delay: Duration,
}

pub(super) enum AnimationDecodeEvent {
    Frame(AnimatedFrame),
    Finished,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AnimatedImageFormat {
    WebP,
    Gif,
    Png,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AnimationScanStatus {
    Unknown,
    Animated(AnimatedImageFormat),
    NotAnimated,
}

pub(super) enum AnimationScanEvent {
    Result {
        generation: u64,
        filename: String,
        status: AnimationScanStatus,
    },
    Finished {
        generation: u64,
    },
}

pub(super) struct AnimatedPage {
    pub(super) frames: Vec<AnimatedFrame>,
    pub(super) current_frame: usize,
    pub(super) next_frame_at: Instant,
    pub(super) decode_complete: bool,
    pub(super) decode_rx: Option<Receiver<AnimationDecodeEvent>>,
}

impl MangaReader {
    pub(super) fn clear_active_animations(&mut self) {
        self.active_animations = std::array::from_fn(|_| None);
        self.active_animation_keys = std::array::from_fn(|_| None);
    }

    pub(super) fn clear_animated_image_cache(&mut self) {
        self.animated_image_byte_cache.clear();
        self.animated_image_first_frame_cache.clear();
    }

    pub(super) fn cancel_animation_scan(&mut self) {
        self.animation_scan_generation = self.animation_scan_generation.wrapping_add(1);
        self.animation_scan_epoch
            .store(self.animation_scan_generation, Ordering::Relaxed);
        self.animation_scan_rx = None;
        self.animation_scan_anchor = None;
    }

    pub(super) fn clear_animation_scan_state(&mut self) {
        self.cancel_animation_scan();
        self.animation_scan_status.clear();
    }

    pub(super) fn animated_image_format(&self, filename: &str) -> Option<AnimatedImageFormat> {
        if !self.config.enable_animated_images {
            return None;
        }
        Self::animated_image_format_unchecked(filename)
    }

    pub(super) fn animated_image_format_unchecked(filename: &str) -> Option<AnimatedImageFormat> {
        let extension = Path::new(filename)
            .extension()?
            .to_string_lossy()
            .to_ascii_lowercase();
        match extension.as_str() {
            "webp" => Some(AnimatedImageFormat::WebP),
            "gif" => Some(AnimatedImageFormat::Gif),
            "png" => Some(AnimatedImageFormat::Png),
            _ => None,
        }
    }

    pub(super) fn frame_delay(frame: &image::Frame) -> Duration {
        let delay = Duration::from(frame.delay());
        if delay.is_zero() {
            Duration::from_millis(100)
        } else {
            delay
        }
    }

    pub(super) fn detect_animation_scan_status(
        format: AnimatedImageFormat,
        bytes: &[u8],
    ) -> Option<AnimationScanStatus> {
        match format {
            AnimatedImageFormat::WebP => {
                let decoder = WebPDecoder::new(Cursor::new(bytes)).ok()?;
                if decoder.has_animation() {
                    Some(AnimationScanStatus::Animated(AnimatedImageFormat::WebP))
                } else {
                    Some(AnimationScanStatus::NotAnimated)
                }
            }
            AnimatedImageFormat::Gif => {
                let mut frames = GifDecoder::new(Cursor::new(bytes)).ok()?.into_frames();
                let _ = frames.next()?.ok()?;
                match frames.next() {
                    Some(Ok(_)) => Some(AnimationScanStatus::Animated(AnimatedImageFormat::Gif)),
                    Some(Err(_)) => None,
                    None => Some(AnimationScanStatus::NotAnimated),
                }
            }
            AnimatedImageFormat::Png => {
                let decoder = PngDecoder::new(Cursor::new(bytes)).ok()?;
                if decoder.is_apng().ok()? {
                    Some(AnimationScanStatus::Animated(AnimatedImageFormat::Png))
                } else {
                    Some(AnimationScanStatus::NotAnimated)
                }
            }
        }
    }

    pub(super) fn spawn_animation_scan_worker(
        source_mode: SourceMode,
        source_path: PathBuf,
        files: Vec<(String, AnimatedImageFormat)>,
        generation: u64,
        epoch: Arc<AtomicU64>,
    ) -> Receiver<AnimationScanEvent> {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for (filename, format) in files {
                if epoch.load(Ordering::Relaxed) != generation {
                    return;
                }
                let Some(bytes) = read_entry(source_mode, &source_path, &filename) else {
                    continue;
                };
                let Some(status) = Self::detect_animation_scan_status(format, &bytes) else {
                    continue;
                };
                if tx
                    .send(AnimationScanEvent::Result {
                        generation,
                        filename,
                        status,
                    })
                    .is_err()
                {
                    return;
                }
            }

            if epoch.load(Ordering::Relaxed) == generation {
                let _ = tx.send(AnimationScanEvent::Finished { generation });
            }
        });
        rx
    }

    pub(super) fn restart_animation_scan_for_current_window(&mut self) {
        self.cancel_animation_scan();

        if !self.config.enable_animated_images
            || self.image_files.is_empty()
            || self.source_mode == SourceMode::Pdf
        {
            return;
        }

        let Some(source_path) = self.zip_path.clone() else {
            return;
        };

        let files: Vec<(String, AnimatedImageFormat)> = self
            .image_files
            .iter()
            .skip(self.current_index)
            .take(ANIMATION_SCAN_AHEAD_PAGES + 1)
            .filter_map(|filename| {
                if !matches!(
                    self.animation_scan_status.get(filename).copied(),
                    Some(AnimationScanStatus::Animated(_)) | Some(AnimationScanStatus::NotAnimated)
                ) {
                    Self::animated_image_format_unchecked(filename)
                        .map(|format| (filename.clone(), format))
                } else {
                    None
                }
            })
            .collect();

        self.animation_scan_anchor = Some(self.current_index);
        if files.is_empty() {
            return;
        }

        let generation = self.animation_scan_generation;
        self.animation_scan_rx = Some(Self::spawn_animation_scan_worker(
            self.source_mode,
            source_path,
            files,
            generation,
            Arc::clone(&self.animation_scan_epoch),
        ));
    }

    pub(super) fn poll_animation_scan(&mut self, ctx: &egui::Context) {
        let mut received_any = false;
        let mut finished = false;

        if let Some(rx) = self.animation_scan_rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(AnimationScanEvent::Result {
                        generation,
                        filename,
                        status,
                    }) => {
                        if generation == self.animation_scan_generation {
                            self.animation_scan_status.insert(filename, status);
                            received_any = true;
                        }
                    }
                    Ok(AnimationScanEvent::Finished { generation }) => {
                        if generation == self.animation_scan_generation {
                            finished = true;
                        }
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }

        if finished {
            self.animation_scan_rx = None;
        }

        if received_any {
            ctx.request_repaint();
        }
    }

    pub(super) fn sync_animation_scan(&mut self, ctx: &egui::Context) {
        self.poll_animation_scan(ctx);

        if !self.config.enable_animated_images || self.image_files.is_empty() {
            self.cancel_animation_scan();
            return;
        }

        if self.animation_scan_anchor != Some(self.current_index) {
            self.restart_animation_scan_for_current_window();
        }
    }

    pub(super) fn decode_animated_image_first_frame(
        &self,
        format: AnimatedImageFormat,
        buffer: Vec<u8>,
        ctx: &egui::Context,
    ) -> Option<(DynamicImage, Option<AnimatedFrame>, Option<Vec<u8>>)> {
        match format {
            AnimatedImageFormat::WebP => {
                let decoder = WebPDecoder::new(Cursor::new(buffer.clone())).ok()?;
                if decoder.has_animation() {
                    let mut frames = decoder.into_frames();
                    let first_frame = frames.next()?.ok()?;
                    let animated_frame = AnimatedFrame {
                        delay: Self::frame_delay(&first_frame),
                        image: self.color_image_from_dynamic(
                            DynamicImage::ImageRgba8(first_frame.clone().into_buffer()),
                            ctx,
                        ),
                    };
                    Some((
                        DynamicImage::ImageRgba8(first_frame.into_buffer()),
                        Some(animated_frame),
                        Some(buffer),
                    ))
                } else {
                    image::load_from_memory_with_format(&buffer, ImageFormat::WebP)
                        .ok()
                        .map(|img| (img, None, None))
                }
            }
            AnimatedImageFormat::Gif => {
                let mut frames = GifDecoder::new(Cursor::new(buffer.clone()))
                    .ok()?
                    .into_frames();
                let first_frame = frames.next()?.ok()?;
                let animated_frame = AnimatedFrame {
                    delay: Self::frame_delay(&first_frame),
                    image: self.color_image_from_dynamic(
                        DynamicImage::ImageRgba8(first_frame.clone().into_buffer()),
                        ctx,
                    ),
                };
                Some((
                    DynamicImage::ImageRgba8(first_frame.into_buffer()),
                    Some(animated_frame),
                    Some(buffer),
                ))
            }
            AnimatedImageFormat::Png => {
                let decoder = PngDecoder::new(Cursor::new(buffer.clone())).ok()?;
                if decoder.is_apng().ok()? {
                    let mut frames = decoder.apng().ok()?.into_frames();
                    let first_frame = frames.next()?.ok()?;
                    let animated_frame = AnimatedFrame {
                        delay: Self::frame_delay(&first_frame),
                        image: self.color_image_from_dynamic(
                            DynamicImage::ImageRgba8(first_frame.clone().into_buffer()),
                            ctx,
                        ),
                    };
                    Some((
                        DynamicImage::ImageRgba8(first_frame.into_buffer()),
                        Some(animated_frame),
                        Some(buffer),
                    ))
                } else {
                    image::load_from_memory_with_format(&buffer, ImageFormat::Png)
                        .ok()
                        .map(|img| (img, None, None))
                }
            }
        }
    }

    pub(super) fn spawn_animated_image_worker(
        format: AnimatedImageFormat,
        bytes: Vec<u8>,
        spec: ImageProcessingSpec,
    ) -> Receiver<AnimationDecodeEvent> {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let process_frame = |frame: image::Frame| -> bool {
                let event = AnimationDecodeEvent::Frame(AnimatedFrame {
                    delay: Self::frame_delay(&frame),
                    image: Self::color_image_from_dynamic_with_spec(
                        DynamicImage::ImageRgba8(frame.into_buffer()),
                        spec,
                    ),
                });
                tx.send(event).is_ok()
            };

            match format {
                AnimatedImageFormat::WebP => {
                    let Ok(decoder) = WebPDecoder::new(Cursor::new(bytes)) else {
                        let _ = tx.send(AnimationDecodeEvent::Failed);
                        return;
                    };
                    if !decoder.has_animation() {
                        let _ = tx.send(AnimationDecodeEvent::Finished);
                        return;
                    }

                    let mut frames = decoder.into_frames();
                    let _ = frames.next();
                    for frame_result in frames {
                        let Ok(frame) = frame_result else {
                            let _ = tx.send(AnimationDecodeEvent::Failed);
                            return;
                        };
                        if !process_frame(frame) {
                            return;
                        }
                    }
                }
                AnimatedImageFormat::Gif => {
                    let Ok(decoder) = GifDecoder::new(Cursor::new(bytes)) else {
                        let _ = tx.send(AnimationDecodeEvent::Failed);
                        return;
                    };
                    let mut frames = decoder.into_frames();
                    let _ = frames.next();
                    for frame_result in frames {
                        let Ok(frame) = frame_result else {
                            let _ = tx.send(AnimationDecodeEvent::Failed);
                            return;
                        };
                        if !process_frame(frame) {
                            return;
                        }
                    }
                }
                AnimatedImageFormat::Png => {
                    let Ok(decoder) = PngDecoder::new(Cursor::new(bytes)) else {
                        let _ = tx.send(AnimationDecodeEvent::Failed);
                        return;
                    };
                    let Ok(is_apng) = decoder.is_apng() else {
                        let _ = tx.send(AnimationDecodeEvent::Failed);
                        return;
                    };
                    if !is_apng {
                        let _ = tx.send(AnimationDecodeEvent::Finished);
                        return;
                    }
                    let Ok(apng) = decoder.apng() else {
                        let _ = tx.send(AnimationDecodeEvent::Failed);
                        return;
                    };
                    let mut frames = apng.into_frames();
                    let _ = frames.next();
                    for frame_result in frames {
                        let Ok(frame) = frame_result else {
                            let _ = tx.send(AnimationDecodeEvent::Failed);
                            return;
                        };
                        if !process_frame(frame) {
                            return;
                        }
                    }
                }
            }

            let _ = tx.send(AnimationDecodeEvent::Finished);
        });
        rx
    }

    pub(super) fn cached_or_load_animated_image_bytes(
        &mut self,
        filename: &str,
    ) -> Option<Vec<u8>> {
        if let Some(bytes) = self.animated_image_byte_cache.get(filename) {
            return Some(bytes.clone());
        }
        let bytes = self.read_source_bytes(filename)?;
        self.animated_image_byte_cache
            .insert(filename.to_owned(), bytes.clone());
        Some(bytes)
    }

    pub(super) fn poll_animation_decode(animation: &mut AnimatedPage) -> bool {
        let mut received_any = false;
        let Some(rx) = animation.decode_rx.as_ref() else {
            return false;
        };
        loop {
            match rx.try_recv() {
                Ok(AnimationDecodeEvent::Frame(frame)) => {
                    animation.frames.push(frame);
                    received_any = true;
                }
                Ok(AnimationDecodeEvent::Finished) => {
                    animation.decode_complete = true;
                    animation.decode_rx = None;
                    return true;
                }
                Ok(AnimationDecodeEvent::Failed) => {
                    animation.decode_complete = true;
                    animation.decode_rx = None;
                    return received_any;
                }
                Err(TryRecvError::Empty) => return received_any,
                Err(TryRecvError::Disconnected) => {
                    animation.decode_complete = true;
                    animation.decode_rx = None;
                    return received_any;
                }
            }
        }
    }

    pub(super) fn ensure_active_animations(&mut self, ctx: &egui::Context) {
        if self.config.page_view_options == PageViewOptions::TopDown {
            self.clear_active_animations();
            return;
        }

        if !self.config.enable_animated_images {
            self.clear_active_animations();
            return;
        }

        // 先頭ページ挿入では、スロット0が白紙、スロット1が1枚目
        let cover_with_blank = self.cover_paired_with_blank();
        for slot in 0..2 {
            let page_index = match (cover_with_blank, slot) {
                (true, 0) => None,
                (true, _) => Some(0),
                (false, _) => Some(self.current_index + slot),
            };
            let desired_key = if self.textures[slot].is_some() {
                page_index
                    .and_then(|index| self.image_files.get(index))
                    .filter(|filename| Self::animated_image_format_unchecked(filename).is_some())
                    .cloned()
            } else {
                None
            };

            let desired_status = desired_key
                .as_ref()
                .and_then(|filename| self.animation_scan_status.get(filename).copied())
                .unwrap_or(AnimationScanStatus::Unknown);

            if self.active_animation_keys[slot] == desired_key
                && !(matches!(desired_status, AnimationScanStatus::Animated(_))
                    && self.active_animations[slot].is_none())
            {
                continue;
            }

            self.active_animation_keys[slot] = desired_key.clone();
            self.active_animations[slot] = None;

            let Some(filename) = desired_key else {
                continue;
            };
            let AnimationScanStatus::Animated(format) = desired_status else {
                continue;
            };
            let first_frame =
                if let Some(frame) = self.animated_image_first_frame_cache.get(&filename) {
                    frame.clone()
                } else {
                    let Some(bytes) = self.cached_or_load_animated_image_bytes(&filename) else {
                        continue;
                    };
                    let Some((_, maybe_first_frame, maybe_cached_bytes)) =
                        self.decode_animated_image_first_frame(format, bytes, ctx)
                    else {
                        continue;
                    };
                    if let Some(cached_bytes) = maybe_cached_bytes {
                        self.animated_image_byte_cache
                            .insert(filename.clone(), cached_bytes);
                    }
                    let Some(frame) = maybe_first_frame else {
                        continue;
                    };
                    self.animated_image_first_frame_cache
                        .insert(filename.clone(), frame.clone());
                    frame
                };
            let Some(bytes) = self.cached_or_load_animated_image_bytes(&filename) else {
                continue;
            };
            let animation = AnimatedPage {
                frames: vec![first_frame.clone()],
                current_frame: 0,
                next_frame_at: Instant::now() + first_frame.delay,
                decode_complete: false,
                decode_rx: Some(Self::spawn_animated_image_worker(
                    format,
                    bytes,
                    self.current_image_processing_spec(ctx),
                )),
            };
            if let Some(texture) = self.textures[slot].as_mut() {
                texture.set(first_frame.image.clone(), egui::TextureOptions::LINEAR);
                self.active_animations[slot] = Some(animation);
            }
        }
    }

    pub(super) fn update_active_animations(&mut self, ctx: &egui::Context) {
        self.ensure_active_animations(ctx);

        let now = Instant::now();
        let mut next_repaint_after: Option<Duration> = None;

        for slot in 0..2 {
            if self.textures[slot].is_none() {
                self.active_animations[slot] = None;
                self.active_animation_keys[slot] = None;
                continue;
            }

            let Some(animation) = self.active_animations[slot].as_mut() else {
                continue;
            };
            let Some(texture) = self.textures[slot].as_mut() else {
                continue;
            };
            let received_new_frames = Self::poll_animation_decode(animation);

            if animation.decode_complete && animation.frames.len() <= 1 {
                continue;
            }

            while now >= animation.next_frame_at {
                let next_index = animation.current_frame + 1;
                if next_index < animation.frames.len() {
                    animation.current_frame = next_index;
                } else if animation.decode_complete && !animation.frames.is_empty() {
                    animation.current_frame = 0;
                } else {
                    break;
                }

                let frame = &animation.frames[animation.current_frame];
                texture.set(frame.image.clone(), egui::TextureOptions::LINEAR);
                animation.next_frame_at = now + frame.delay;
            }

            let repaint_after = if animation.current_frame + 1 < animation.frames.len()
                || animation.decode_complete
            {
                animation.next_frame_at.saturating_duration_since(now)
            } else {
                Duration::from_millis(if received_new_frames { 1 } else { 16 })
            };
            next_repaint_after = Some(
                next_repaint_after
                    .map(|current| current.min(repaint_after))
                    .unwrap_or(repaint_after),
            );
        }

        if let Some(repaint_after) = next_repaint_after {
            ctx.request_repaint_after(repaint_after.max(Duration::from_millis(1)));
        }
    }
}
