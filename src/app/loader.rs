use super::MangaReader;
use super::animation::AnimationScanStatus;
use crate::config::{PageViewOptions, SourceMode};
use crate::engine::imaging::{
    ImageProcessingSpec, resize_for_display, strip_adobe_app14_if_invalid,
};
use crate::engine::layout::is_portrait;
use crate::engine::library::{natural_sort_strings, scan_folder};
use crate::engine::source::{Listing, SourceReader, list_images, read_entry};
use crate::localize::tr;
use eframe::egui;
use image::{DynamicImage, ImageFormat};
use pdfium_render::prelude::{Pdfium, Pixels};
use std::path::{Path, PathBuf};
use std::time::Instant;

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

        // Pdfium::default() panics when no library is found. Look in the working
        // directory (as it does), next to the executable, then system-wide, and
        // report a missing library instead.
        let mut dirs = vec![PathBuf::from("./")];
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            dirs.push(exe_dir);
        }
        let bindings = dirs
            .iter()
            .find_map(|dir| {
                Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir)).ok()
            })
            .or_else(|| Pdfium::bind_to_system_library().ok());
        let Some(bindings) = bindings else {
            eprintln!("pdfium library not found; PDF files cannot be opened.");
            return None;
        };
        let pdfium = Box::leak(Box::new(Pdfium::new(bindings)));
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
        let processed_img = resize_for_display(img, &spec);

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
            println!("filter: {:?}", spec.resize_method.to_filter());
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
        read_entry(self.source_mode, source_path, filename)
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

        let mut reader = SourceReader::open(self.source_mode, &source_path);

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
                    let bytes = reader.read(filename);

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
        let extension = path
            .extension()
            .map_or("".to_string(), |ext| ext.to_string_lossy().to_lowercase());
        let listing = if extension == "pdf" {
            let mut images = Vec::new();
            if let Some(page_count) = self.ensure_pdf_document_loaded(&path) {
                for i in 0..page_count {
                    // We use a virtual naming scheme for PDF pages in our image_files list
                    images.push(format!("pdf_page_{}", i));
                }
            } else if self.pdfium.is_none() {
                self.show_fading_error(tr("error.pdfium_missing"));
                return;
            }
            Listing {
                mode: SourceMode::Pdf,
                root: path.clone(),
                images,
                start_at: None,
            }
        } else {
            match list_images(&path) {
                Some(listing) => listing,
                None => return,
            }
        };
        self.source_mode = listing.mode;
        let target_path = listing.root;
        let start_at_filename = listing.start_at;
        let mut images = listing.images;

        natural_sort_strings(&mut images);

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
