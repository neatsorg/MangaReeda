use super::MangaReader;
use super::animation::AnimationScanStatus;
use super::navigation::is_portrait;
use crate::config::{ImageSizingMode, PageViewOptions, ResizeMethod, SourceMode};
use crate::localize::tr;
use crate::utils::{scan_folder, strip_adobe_app14_if_invalid, windows_natural_sort_strings};
use eframe::egui;
use image::{DynamicImage, ImageFormat};
use pdfium_render::prelude::{Pdfium, Pixels};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy)]
pub(super) struct ImageProcessingSpec {
    pub(super) resize_method: ResizeMethod,
    pub(super) image_sizing_mode: ImageSizingMode,
    pub(super) transparency_support: bool,
    pub(super) zoom_factor: f32,
    pub(super) container_size: [f32; 2],
}

impl MangaReader {
    pub(super) fn reload_current_image(&mut self, ctx: &egui::Context) {
        if self.image_files.is_empty() {
            return;
        }

        self.stop_auto_scroll();
        self.texture_cache.clear();
        self.clear_animated_image_cache();
        self.clear_animation_scan_state();
        self.clear_active_animations();
        self.reset_buffer();
        self.reset_pan();
        self.last_buffered_index = None;

        if self.config.page_view_options == PageViewOptions::TopDown {
            self.textures = [None, None];
            self.ensure_top_down_loaded_around(self.current_index, ctx);
            let preload_count = self.image_files.len().min(5);
            for index in 0..preload_count {
                self.ensure_top_down_texture_loaded(index, ctx);
            }
        } else {
            self.load_current_pair(ctx);
        }

        self.page_indicator_time = Some(Instant::now());
        ctx.request_repaint();
    }

    pub(super) fn clear_pdf_document_cache(&mut self) {
        self.pdf_document = None;
        self.pdf_document_path = None;
    }

    pub(super) fn pdfium_instance(&mut self) -> Option<&'static Pdfium> {
        if let Some(pdfium) = self.pdfium {
            return Some(pdfium);
        }

        let pdfium = Box::leak(Box::new(Pdfium::default()));
        self.pdfium = Some(pdfium);
        Some(pdfium)
    }

    pub(super) fn ensure_pdf_document_loaded(&mut self, path: &Path) -> Option<usize> {
        if self.pdf_document_path.as_deref() == Some(path) {
            return self
                .pdf_document
                .as_ref()
                .map(|doc| usize::from(doc.pages().len()));
        }

        self.clear_pdf_document_cache();

        let doc = self
            .pdfium_instance()?
            .load_pdf_from_file(path, None)
            .ok()?;
        let page_count = usize::from(doc.pages().len());

        self.pdf_document = Some(doc);
        self.pdf_document_path = Some(path.to_path_buf());

        Some(page_count)
    }

    pub(super) fn open_file_dialog(&mut self) {
        let now = std::time::Instant::now();
        if now.duration_since(self.last_dialog_time) > std::time::Duration::from_millis(500) {
            self.last_dialog_time = now;
            if !self.is_dialog_open {
                self.is_dialog_open = true;
                let sender = self.dialog_tx.clone();
                let filter_label = tr("dialog.manga_files").to_owned();

                std::thread::spawn(move || {
                    let file = rfd::FileDialog::new()
                        .add_filter(
                            &filter_label,
                            &[
                                "zip", "cbz", "cbr", "rar", "png", "jpg", "jpeg", "bmp", "webp",
                                "gif", "tiff", "tga", "avif", "pdf",
                            ],
                        )
                        .pick_file();

                    let _ = sender.send(file);
                });
            }
        }
    }

    pub(super) fn read_source_bytes_from(
        source_mode: SourceMode,
        source_path: &Path,
        filename: &str,
    ) -> Option<Vec<u8>> {
        if source_mode == SourceMode::Folder {
            return fs::read(filename).ok();
        }
        if source_mode == SourceMode::Zip {
            let file = File::open(source_path).ok()?;
            let mut archive = zip::ZipArchive::new(file).ok()?;
            let mut entry = archive.by_name(filename).ok()?;
            let mut buffer = Vec::new();
            entry.read_to_end(&mut buffer).ok()?;
            return Some(buffer);
        }
        if source_mode == SourceMode::Rar {
            return unrar::Archive::new(source_path)
                .open_for_processing()
                .ok()
                .and_then(|rar_archive| {
                    let mut cursor = rar_archive.read_header().ok().flatten();
                    loop {
                        match cursor {
                            Some(e) => {
                                let current_name = e.entry().filename.to_str();
                                if let Some(name_str) = current_name {
                                    if name_str == filename {
                                        break e.read().ok().map(|(bytes, _arc)| bytes);
                                    }
                                    cursor = e
                                        .skip()
                                        .ok()
                                        .and_then(|arc| arc.read_header().ok().flatten());
                                } else {
                                    cursor = e
                                        .skip()
                                        .ok()
                                        .and_then(|arc| arc.read_header().ok().flatten());
                                }
                            }
                            None => break None,
                        }
                    }
                });
        }
        None
    }

    pub(super) fn current_image_processing_spec(&self, ctx: &egui::Context) -> ImageProcessingSpec {
        let container_size = self.effective_image_container_size(ctx);
        ImageProcessingSpec {
            resize_method: self.config.resize_method,
            image_sizing_mode: self.config.image_sizing_mode,
            transparency_support: self.config.transparency_support,
            zoom_factor: self.zoom_factor,
            container_size: [container_size.x, container_size.y],
        }
    }

    pub(super) fn color_image_from_dynamic_with_spec(
        img: DynamicImage,
        spec: ImageProcessingSpec,
    ) -> egui::ColorImage {
        let resize_start = Instant::now();
        let filter = spec.resize_method.to_filter();
        let processed_img = if let Some(filter_type) = filter {
            if spec.image_sizing_mode == ImageSizingMode::OriginalSize {
                img
            } else {
                let container_size = egui::vec2(spec.container_size[0], spec.container_size[1]);
                let aspect_ratio = img.width() as f32 / img.height() as f32;
                let factor = if (spec.zoom_factor - 1.0).abs() > 0.01 {
                    3
                } else {
                    1
                };
                let (target_w, target_h) = match spec.image_sizing_mode {
                    ImageSizingMode::FitHeight => {
                        let target_h = container_size.y.max(1.0) as u32;
                        let target_w = (target_h as f32 * aspect_ratio) as u32;
                        (target_w, target_h)
                    }
                    ImageSizingMode::FitWidth => {
                        let target_w = container_size.x.max(1.0) as u32;
                        let target_h = (target_w as f32 / aspect_ratio) as u32;
                        (target_w, target_h)
                    }
                    ImageSizingMode::FitBoth => {
                        let width_scale = container_size.x.max(1.0) / img.width() as f32;
                        let height_scale = container_size.y.max(1.0) / img.height() as f32;
                        let scale = width_scale.min(height_scale);
                        let target_w = (img.width() as f32 * scale) as u32;
                        let target_h = (img.height() as f32 * scale) as u32;
                        (target_w, target_h)
                    }
                    ImageSizingMode::OriginalSize => unreachable!(),
                };
                img.resize(
                    target_w.max(1) * factor,
                    target_h.max(1) * factor,
                    filter_type,
                )
            }
        } else {
            img
        };

        let _resize_time = resize_start.elapsed();
        let process_start = Instant::now();
        let size = [processed_img.width() as _, processed_img.height() as _];
        let color_img = if spec.transparency_support {
            egui::ColorImage::from_rgba_unmultiplied(
                size,
                processed_img.to_rgba8().as_flat_samples().as_slice(),
            )
        } else {
            egui::ColorImage::from_rgb(size, processed_img.to_rgb8().as_raw())
        };
        let _process_time = process_start.elapsed();

        #[cfg(debug_assertions)]
        {
            println!("----------------------------------");
            println!("resize_time: {:?}", _resize_time);
            println!("process_time: {:?}", _process_time);
            println!("total: {:?}", _process_time + _resize_time);
            println!("filter: {:?}", filter);
            println!("----------------------------------");
        }

        color_img
    }

    pub(super) fn color_image_from_dynamic(
        &self,
        img: DynamicImage,
        ctx: &egui::Context,
    ) -> egui::ColorImage {
        let mut spec = self.current_image_processing_spec(ctx);
        if self.automatic_page_direction().is_some() {
            // Preloaded images and animation first frames need the resolution of
            // their own layout, even when the current page uses a different one.
            let rect = ctx.content_rect();
            spec.container_size = if is_portrait([img.width(), img.height()]) {
                [
                    ((rect.width() * 0.5) - self.config.spread_center_offset.abs()).max(1.0),
                    rect.height(),
                ]
            } else {
                [rect.width(), rect.height()]
            };
        }
        Self::color_image_from_dynamic_with_spec(img, spec)
    }

    pub(super) fn read_source_bytes(&self, filename: &str) -> Option<Vec<u8>> {
        let source_path = self.zip_path.as_ref()?;
        Self::read_source_bytes_from(self.source_mode, source_path, filename)
    }

    pub(super) fn load_pair(
        &mut self,
        start_idx: usize,
        ctx: &egui::Context,
    ) -> [Option<egui::TextureHandle>; 2] {
        // no source path set yet
        if self.zip_path == None {
            {
                return [None, None];
            }
        }

        let mut pair: [Option<egui::TextureHandle>; 2] = [None, None];
        let source_path = self.zip_path.clone().unwrap();

        let mut archive = if self.source_mode == SourceMode::Zip {
            let path = source_path.clone();
            File::open(path)
                .ok()
                .and_then(|f| zip::ZipArchive::new(f).ok())
        } else {
            None
        };

        for i in 0..2 {
            let current_target = start_idx + i;
            if let Some(filename) = self.image_files.get(current_target) {
                if let Some(handle) = self.texture_cache.get(filename) {
                    pair[i] = Some(handle.clone());
                    continue;
                }

                if self.source_mode == SourceMode::Pdf {
                    let dynamic_image = self.render_pdf_page(current_target, ctx);
                    if let Some(img) = dynamic_image {
                        pair[i] = self.load_texture(img, filename.clone(), ctx);
                    }
                } else {
                    let bytes = if self.source_mode == SourceMode::Folder {
                        fs::read(filename).ok() // Load directly from path
                    } else if let Some(ref mut arc) = archive {
                        arc.by_name(filename).ok().and_then(|mut f| {
                            let mut b = Vec::new();
                            f.read_to_end(&mut b).ok().map(|_| b)
                        })
                    } else if self.source_mode == SourceMode::Rar {
                        unrar::Archive::new(&source_path)
                            .open_for_processing()
                            .ok()
                            .and_then(|rar_achive| {
                                let mut cursor = rar_achive.read_header().ok().flatten();
                                loop {
                                    match cursor {
                                        Some(e) => {
                                            // Use .entry() before reference filename
                                            let current_name = e.entry().filename.to_str();

                                            if let Some(name_str) = current_name {
                                                if name_str == filename {
                                                    break e.read().ok().map(|(bytes, _arc)| bytes);
                                                } else {
                                                    cursor = e.skip().ok().and_then(|arc| {
                                                        arc.read_header().ok().flatten()
                                                    });
                                                }
                                            } else {
                                                // Filename wasn't valid UTF-8, skip it
                                                cursor = e.skip().ok().and_then(|arc| {
                                                    arc.read_header().ok().flatten()
                                                });
                                            }
                                        }
                                        None => break None,
                                    }
                                }
                            })
                    } else {
                        None
                    };

                    if let Some(mut buffer) = bytes {
                        if self.config.enable_auto_image_byte_fix {
                            buffer = strip_adobe_app14_if_invalid(&buffer);
                        }
                        let scan_status = self
                            .animation_scan_status
                            .get(filename)
                            .copied()
                            .unwrap_or(AnimationScanStatus::Unknown);
                        let animated_format = match scan_status {
                            AnimationScanStatus::Animated(format) => Some(format),
                            AnimationScanStatus::NotAnimated => None,
                            AnimationScanStatus::Unknown => self.animated_image_format(filename),
                        };

                        if let Some(format) = animated_format {
                            if let Some((img, first_frame, animated_bytes)) =
                                self.decode_animated_image_first_frame(format, buffer, ctx)
                            {
                                if let Some(frame) = first_frame {
                                    self.animation_scan_status.insert(
                                        filename.clone(),
                                        AnimationScanStatus::Animated(format),
                                    );
                                    self.animated_image_first_frame_cache
                                        .insert(filename.clone(), frame);
                                } else {
                                    self.animation_scan_status
                                        .insert(filename.clone(), AnimationScanStatus::NotAnimated);
                                }
                                if let Some(animated_bytes) = animated_bytes {
                                    self.animated_image_byte_cache
                                        .insert(filename.clone(), animated_bytes);
                                }
                                pair[i] = self.load_texture(img, filename.clone(), ctx);
                            }
                        } else {
                            match image::guess_format(&buffer) {
                                Ok(format) => {
                                    if let Ok(img) =
                                        image::load_from_memory_with_format(&buffer, format)
                                    {
                                        pair[i] = self.load_texture(img, filename.clone(), ctx);
                                    }
                                }
                                Err(_) => {
                                    // Fallback: If guessing fails, try loading as TGA
                                    // since TGA is often the one that fails detection.
                                    if let Ok(img) = image::load_from_memory_with_format(
                                        &buffer,
                                        ImageFormat::Tga,
                                    ) {
                                        pair[i] = self.load_texture(img, filename.clone(), ctx);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if self.automatic_page_direction().is_some() && self.automatic_page_count(start_idx) == 1 {
            pair[1] = None;
        }
        pair
    }

    /// Helper to render a specific page
    pub(super) fn render_pdf_page(
        &self,
        index: usize,
        ctx: &egui::Context,
    ) -> Option<DynamicImage> {
        let source_path = self.zip_path.as_ref()?;
        let doc = match self.pdf_document.as_ref() {
            Some(doc) if self.pdf_document_path.as_deref() == Some(source_path.as_path()) => doc,
            _ => return None,
        };

        let page = match doc.pages().get(index as u16) {
            Ok(page) => page,
            Err(_) => return None,
        };
        let width_inch = page.width().value;
        let height_inch = page.height().value;

        let screen_size = ctx.content_rect();
        let target_h = screen_size.height();
        let h_ratio = target_h / height_inch;
        let target_w = width_inch * h_ratio;

        // Render at 300 DPI or based on screen height for clarity
        let bitmap = page
            .render(target_w as Pixels, target_h as Pixels, None)
            .ok()?;
        Some(bitmap.as_image()) // pdfium-render integrates with the 'image' crate
    }

    pub(super) fn load_texture(
        &mut self,
        img: DynamicImage,
        cache_name: String,
        ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        let dimensions = [img.width(), img.height()];
        self.image_dimensions.insert(cache_name.clone(), dimensions);
        let color_img = self.color_image_from_dynamic(img, ctx);
        let handle = ctx.load_texture(
            &cache_name.clone(),
            color_img,
            egui::TextureOptions::LINEAR, // Smooth scaling
        );
        if self.config.enable_single_file_caching {
            self.texture_cache
                .insert(cache_name.clone(), handle.clone());
        }
        Some(handle)
    }

    pub(super) fn load_source(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.stop_auto_scroll();
        self.clear_pdf_document_cache();
        let mut target_path = path.clone();
        let mut start_at_filename: Option<String> = None;

        let mut images = Vec::new();
        let exts = [
            "png", "jpg", "jpeg", "bmp", "webp", "gif", "tiff", "tga", "avif",
        ];
        let extension = path
            .extension()
            .map_or("".to_string(), |ext| ext.to_string_lossy().to_lowercase());
        match extension.as_str() {
            "zip" | "cbz" => {
                let file = match File::open(&target_path) {
                    Ok(f) => f,
                    Err(_) => return,
                };
                if let Ok(mut archive) = zip::ZipArchive::new(file) {
                    for i in 0..archive.len() {
                        if let Ok(f) = archive.by_index(i) {
                            let name = f.name().to_lowercase();
                            if exts.iter().any(|&e| name.ends_with(&format!(".{}", e))) {
                                images.push(f.name().to_string());
                            }
                        }
                    }
                    self.source_mode = SourceMode::Zip;
                }
                self.source_mode = SourceMode::Zip;
                // ... existing zip logic
            }
            "cbr" | "rar" => {
                self.source_mode = SourceMode::Rar;
                if let Ok(archive) = unrar::Archive::new(&path).open_for_listing() {
                    for entry in archive {
                        if let Ok(e) = entry {
                            // Convert Option<&str> to String safely
                            if let Some(name_str) = e.filename.to_str() {
                                let name = name_str.to_string();
                                // Check if it's an image extension
                                if exts
                                    .iter()
                                    .any(|&e_ext| name.to_lowercase().ends_with(e_ext))
                                {
                                    images.push(name);
                                }
                            }
                        }
                    }
                }
            }
            "pdf" => {
                // --- PDF MODE ---
                match self.ensure_pdf_document_loaded(&path) {
                    Some(page_count) => {
                        for i in 0..page_count {
                            // We use a virtual naming scheme for PDF pages in our image_files list
                            images.push(format!("pdf_page_{}", i));
                        }
                    }
                    None => {}
                }
                self.source_mode = SourceMode::Pdf;
            }
            _ => {
                if path.is_file() {
                    // Pivot: Use the folder containing this image as the source
                    if let Some(parent) = path.parent() {
                        start_at_filename = Some(path.to_string_lossy().to_string());
                        target_path = parent.to_path_buf();
                    }
                }

                if target_path.is_dir() {
                    // --- FOLDER MODE ---
                    if let Ok(entries) = fs::read_dir(&target_path) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_file()
                                && exts.iter().any(|&e| {
                                    p.extension().map_or(false, |ext| {
                                        ext.to_string_lossy().to_lowercase() == e
                                    })
                                })
                            {
                                images.push(p.to_string_lossy().to_string());
                            }
                        }
                    }
                }
                self.source_mode = SourceMode::Folder;
            }
        }

        windows_natural_sort_strings(&mut images);

        if images.is_empty() {
            let msg = tr("error.no_images_found").to_owned();
            self.show_fading_error(&msg);
        } else {
            self.reset_buffer();
            self.texture_cache.clear();
            self.image_dimensions.clear();
            self.clear_animated_image_cache();
            self.clear_animation_scan_state();
            self.clear_active_animations();
            self.reset_pan();

            // If we opened a specific image, find its index in the sorted list
            self.current_index = if let Some(target_name) = start_at_filename {
                images.iter().position(|r| r == &target_name).unwrap_or(0)
            } else {
                0
            };

            self.zip_path = Some(target_path.clone());
            self.image_files = images;

            // Scan parent for Next/Prev file navigation
            self.all_zips_in_folder = scan_folder(&target_path.parent().unwrap_or(Path::new("")));

            if let Some(file_name) = target_path.file_name() {
                self.zip_name_display =
                    Some((file_name.to_string_lossy().to_string(), Instant::now()));
            }

            self.restart_animation_scan_for_current_window();
            self.load_current_pair(ctx);
            self.reset_top_down_buffer();
            if self.config.page_view_options == PageViewOptions::TopDown {
                self.ensure_top_down_loaded_around(self.current_index, ctx);
                for index in 0..self.image_files.len().min(5) {
                    self.ensure_top_down_texture_loaded(index, ctx);
                }
            }
        }

        self.page_indicator_time = Some(Instant::now());
    }
}
