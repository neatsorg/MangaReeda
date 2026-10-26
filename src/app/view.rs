use super::{MangaReader, ZOOM_MAX, ZOOM_MIN, ZOOM_RELOAD_DELAY, ZOOM_STEP};
use crate::config::{ImageSizingMode, PageViewOptions};
use eframe::egui;
use egui::Rect;
use std::time::Instant;

impl MangaReader {
    pub(super) fn image_panel_background_color(&self) -> egui::Color32 {
        let [r, g, b, a] = self.config.image_panel_background;
        egui::Color32::from_rgba_unmultiplied(r, g, b, a)
    }

    pub(super) fn reset_pan(&mut self) {
        self.pan_offset = egui::Vec2::ZERO;
    }

    pub(super) fn reload_zoom_textures(&mut self, ctx: &egui::Context) {
        self.reset_buffer();
        self.texture_cache.clear();
        self.load_current_pair(ctx);
    }

    pub(super) fn schedule_zoom_texture_reload(&mut self, ctx: &egui::Context) {
        self.last_zoom_change_at = Some(Instant::now());
        self.zoom_reload_pending = true;
        ctx.request_repaint_after(ZOOM_RELOAD_DELAY);
    }

    pub(super) fn reload_zoom_textures_if_settled(&mut self, ctx: &egui::Context) {
        if !self.zoom_reload_pending {
            return;
        }
        if self.zip_path.is_none() || self.image_files.is_empty() {
            self.zoom_reload_pending = false;
            self.last_zoom_change_at = None;
            return;
        }

        if let Some(last_zoom_change_at) = self.last_zoom_change_at {
            let elapsed = last_zoom_change_at.elapsed();
            if elapsed >= ZOOM_RELOAD_DELAY {
                self.reload_zoom_textures(ctx);
                self.zoom_reload_pending = false;
                self.last_zoom_change_at = None;
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(ZOOM_RELOAD_DELAY - elapsed);
            }
        }
    }

    pub(super) fn apply_zoom_factor(
        &mut self,
        new_zoom_factor: f32,
        ctx: &egui::Context,
        anchor_to_pointer: bool,
    ) {
        let old_zoom_factor = self.zoom_factor;
        let new_zoom_factor = new_zoom_factor.clamp(ZOOM_MIN, ZOOM_MAX);
        if (new_zoom_factor - old_zoom_factor).abs() <= f32::EPSILON {
            return;
        }

        let rect = self
            .last_page_view_rect
            .unwrap_or_else(|| ctx.content_rect());
        if anchor_to_pointer
            && let Some(pointer_pos) = ctx.pointer_hover_pos()
            && rect.contains(pointer_pos)
        {
            let scale_ratio = new_zoom_factor / old_zoom_factor;
            if self.config.page_view_options == PageViewOptions::TopDown {
                let relative_y = pointer_pos.y - rect.center().y - self.top_down_scroll_offset;
                self.top_down_scroll_offset += relative_y * (1.0 - scale_ratio);
                self.clamp_top_down_scroll_offset();
            } else {
                let relative_pos = pointer_pos - rect.center() - self.pan_offset;
                self.pan_offset += relative_pos * (1.0 - scale_ratio);
            }
        } else if (new_zoom_factor - 1.0).abs() <= 0.01 {
            self.reset_pan();
        }

        self.zoom_factor = new_zoom_factor;
        self.schedule_zoom_texture_reload(ctx);
        ctx.request_repaint();
    }

    pub(super) fn zoom_in(&mut self, ctx: &egui::Context) {
        self.apply_zoom_factor(self.zoom_factor * ZOOM_STEP, ctx, true);
    }

    pub(super) fn zoom_out(&mut self, ctx: &egui::Context) {
        self.apply_zoom_factor(self.zoom_factor / ZOOM_STEP, ctx, true);
    }

    pub(super) fn effective_image_container_size(&self, ctx: &egui::Context) -> egui::Vec2 {
        let content_rect = ctx.content_rect();
        if self.is_single_page() || (self.is_shifted && self.current_index == 0) {
            content_rect.size()
        } else {
            egui::vec2(
                ((content_rect.width() * 0.5) - self.config.spread_center_offset.abs()).max(1.0),
                content_rect.height(),
            )
        }
    }

    pub(super) fn image_draw_size(
        &self,
        tex: &egui::TextureHandle,
        container_size: egui::Vec2,
        zoom_factor: f32,
    ) -> egui::Vec2 {
        let tex_size = tex.size_vec2();
        let aspect_ratio = tex_size.x / tex_size.y;

        match self.config.image_sizing_mode {
            ImageSizingMode::FitHeight => {
                let height = container_size.y * zoom_factor;
                egui::vec2(height * aspect_ratio, height)
            }
            ImageSizingMode::FitWidth => {
                let width = container_size.x * zoom_factor;
                egui::vec2(width, width / aspect_ratio)
            }
            ImageSizingMode::FitBoth => {
                let scale = (container_size.x / tex_size.x).min(container_size.y / tex_size.y);
                tex_size * scale * zoom_factor
            }
            ImageSizingMode::OriginalSize => tex_size * zoom_factor,
        }
    }

    pub(super) fn top_down_image_draw_size(
        &self,
        tex: &egui::TextureHandle,
        container_size: egui::Vec2,
    ) -> egui::Vec2 {
        let tex_size = tex.size_vec2();
        let aspect_ratio = tex_size.x / tex_size.y;
        match self.config.image_sizing_mode {
            ImageSizingMode::FitHeight => {
                let height = container_size.y.max(1.0) * self.zoom_factor;
                egui::vec2(height * aspect_ratio, height)
            }
            ImageSizingMode::FitWidth => {
                let width = container_size.x.max(1.0) * self.zoom_factor;
                egui::vec2(width, width / aspect_ratio)
            }
            ImageSizingMode::FitBoth => {
                let scale = (container_size.x.max(1.0) / tex_size.x)
                    .min(container_size.y.max(1.0) / tex_size.y);
                tex_size * scale * self.zoom_factor
            }
            ImageSizingMode::OriginalSize => tex_size * self.zoom_factor,
        }
    }

    pub(super) fn reset_top_down_buffer(&mut self) {
        self.top_down_textures = vec![None; self.image_files.len()];
        self.top_down_scroll_offset = 0.0;
    }

    pub(super) fn ensure_top_down_texture_loaded(&mut self, index: usize, ctx: &egui::Context) {
        if index >= self.image_files.len() {
            return;
        }
        if self.top_down_textures.len() != self.image_files.len() {
            self.reset_top_down_buffer();
        }
        if self.top_down_textures[index].is_some() {
            return;
        }

        let pair = self.load_pair(index, ctx);
        self.top_down_textures[index] = pair[0].clone();
        if index + 1 < self.top_down_textures.len() {
            self.top_down_textures[index + 1] = pair[1].clone();
        }
    }

    pub(super) fn ensure_top_down_loaded_around(&mut self, index: usize, ctx: &egui::Context) {
        if self.config.page_view_options != PageViewOptions::TopDown {
            return;
        }
        if self.image_files.is_empty() {
            return;
        }
        let first = index.saturating_sub(2);
        let last = index
            .saturating_add(2)
            .min(self.image_files.len().saturating_sub(1));
        for page_index in first..=last {
            self.ensure_top_down_texture_loaded(page_index, ctx);
        }
    }

    pub(super) fn top_down_texture(&self, index: usize) -> Option<&egui::TextureHandle> {
        self.top_down_textures.get(index)?.as_ref()
    }

    pub(super) fn clamp_top_down_scroll_offset(&mut self) {
        if self.current_index == 0 && self.top_down_scroll_offset > 0.0 {
            self.top_down_scroll_offset = 0.0;
        }
        if self.current_index + 1 >= self.image_files.len() && self.top_down_scroll_offset < 0.0 {
            self.top_down_scroll_offset = 0.0;
        }
    }

    pub(super) fn double_page_needs_drag(&self, rect: Rect) -> bool {
        let page_size = egui::vec2(rect.width() * 0.5, rect.height());
        self.textures[1]
            .as_ref()
            .map(|tex| self.image_draw_size(tex, page_size, self.zoom_factor))
            .is_some_and(|size| size.x > page_size.x || size.y > page_size.y)
            || self.textures[0]
                .as_ref()
                .map(|tex| self.image_draw_size(tex, page_size, self.zoom_factor))
                .is_some_and(|size| size.x > page_size.x || size.y > page_size.y)
    }

    pub(super) fn create_image_rect(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        visible_rect: Rect,
        hit_id: &str,
        tex_index: usize,
        align: egui::Align,
        image_size: egui::Vec2,
    ) -> egui::Response {
        ui.allocate_ui_at_rect(rect, |ui| {
            let resp = ui.interact(visible_rect, ui.id().with(hit_id), egui::Sense::click());

            if let Some(tex) = &self.textures[tex_index] {
                let previous_clip_rect = ui.clip_rect();
                ui.set_clip_rect(previous_clip_rect.intersect(visible_rect));
                if self.config.image_sizing_mode == ImageSizingMode::FitBoth {
                    let image_x = match align {
                        egui::Align::RIGHT => rect.right() - image_size.x,
                        egui::Align::Center => rect.center().x - image_size.x * 0.5,
                        egui::Align::LEFT => rect.left(),
                    };
                    let image_y = rect.center().y - image_size.y * 0.5;
                    ui.painter().image(
                        tex.id(),
                        egui::Rect::from_min_size(egui::pos2(image_x, image_y), image_size),
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    let layout = egui::Layout::top_down(align);
                    ui.with_layout(layout, |ui| {
                        ui.add(
                            egui::Image::new(tex)
                                .fit_to_exact_size(image_size)
                                .maintain_aspect_ratio(true),
                        );
                    });
                }
                ui.set_clip_rect(previous_clip_rect);
            }
            resp
        })
        .inner
    }

    pub(super) fn paint_image_clipped(
        &self,
        ui: &egui::Ui,
        tex: &egui::TextureHandle,
        image_rect: Rect,
        visible_rect: Rect,
    ) {
        ui.painter()
            .with_clip_rect(ui.clip_rect().intersect(visible_rect))
            .image(
                tex.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
    }
}
