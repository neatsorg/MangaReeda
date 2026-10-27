use super::{MangaReader, ZOOM_MAX, ZOOM_MIN, ZOOM_RELOAD_DELAY, ZOOM_STEP};

/// 先頭ページ挿入の白紙のテクスチャの長辺（ピクセル）
const COVER_BLANK_MAX_SIDE: f32 = 64.0;
/// 白紙のテクスチャの名前。ページのテクスチャはファイル名なので重ならない
const BLANK_TEXTURE_NAME: &str = "mangareeda:blank";
/// ルーペの円を近似する多角形の頂点数
const LOUPE_SEGMENTS: usize = 96;
use crate::config::{ImageSizingMode, PageViewOptions};
use crate::engine::imaging::{matched_height, shrink_only_scale};
use crate::engine::loupe;
use crate::engine::scroll::{self, Span};
use eframe::egui;
use egui::Rect;
use std::time::Instant;

impl MangaReader {
    pub(super) fn image_panel_background_color(&self) -> egui::Color32 {
        let [r, g, b, a] = self.config.image_panel_background;
        egui::Color32::from_rgba_unmultiplied(r, g, b, a)
    }

    /// 表示位置を戻す。はみ出すページは、次に描くときに読み始めの位置へ合わせる
    pub(super) fn reset_pan(&mut self) {
        self.pan_offset = egui::Vec2::ZERO;
        self.pan_to_start_pending = true;
    }

    /// reset_pan のあと、ページが読み込まれていれば読み始めの位置へ合わせる
    pub(super) fn apply_pending_pan_start(&mut self) {
        if !self.pan_to_start_pending || self.textures[0].is_none() {
            return;
        }
        self.pan_to_start_pending = false;
        if let Some((x, y)) = self.scroll_spans() {
            let (px, py) = scroll::start_position(x, y, self.reads_right_to_left());
            self.pan_offset = egui::vec2(px, py);
        }
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
        if self.is_single_page() || self.cover_shown_alone() {
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
        self.draw_size_of(
            tex.size_vec2(),
            self.original_size(tex),
            container_size,
            zoom_factor,
        )
    }

    /// テクスチャの大きさ `tex_size`、元画像の大きさ `original` の画像（またはその一部）の描画サイズ
    fn draw_size_of(
        &self,
        tex_size: egui::Vec2,
        original: egui::Vec2,
        container_size: egui::Vec2,
        zoom_factor: f32,
    ) -> egui::Vec2 {
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
            // 1枚だけを見るときの「縦サイズを揃える」は幅と高さに合わせると同じ
            ImageSizingMode::FitBoth | ImageSizingMode::MatchHeight => {
                let scale = (container_size.x / tex_size.x).min(container_size.y / tex_size.y);
                tex_size * scale * zoom_factor
            }
            ImageSizingMode::ShrinkOnly => {
                let scale = shrink_only_scale(original.into(), container_size.into());
                original * scale * zoom_factor
            }
            ImageSizingMode::OriginalSize => tex_size * zoom_factor,
        }
    }

    /// 1ページ表示で、今のページを左右に分けて見せているとき（MM-VIEW-05）、右半分を見せているか。
    /// 分けていなければ None。分けるのは、自動単ページ表示がオンで、1ページ表示で、横長のページ
    pub(super) fn split_half_shows_right(&self) -> Option<bool> {
        if !self.config.split_landscape_pages
            || self.config.page_view_options != PageViewOptions::Single
            || !self
                .page_dimensions(self.current_index)
                .is_some_and(|[w, h]| w > h)
        {
            return None;
        }
        // 先に読む半分は、右開きなら右、左開きなら左
        let second = self.split_second_half_of == Some(self.current_index);
        Some(self.config.reads_right_to_left() != second)
    }

    /// 1ページ表示で今のページを描く範囲。戻り値は（画像全体を描く範囲, 見せる範囲）。
    /// 左右に分けて見せるときは、見せる半分が `center` に来るよう画像全体をずらし、半分だけを見せる
    pub(super) fn single_page_rects(
        &self,
        center: egui::Pos2,
        container: egui::Vec2,
    ) -> Option<(Rect, Rect)> {
        let tex = self.textures[0].as_ref()?;
        let Some(shows_right) = self.split_half_shows_right() else {
            let rect = Rect::from_center_size(
                center,
                self.image_draw_size(tex, container, self.zoom_factor),
            );
            return Some((rect, rect));
        };
        // 分割位置は幅の半分（ミーヤの既定 HalfPageVPos=@width/2）
        let half = |size: egui::Vec2| egui::vec2(size.x * 0.5, size.y);
        let half_size = self.draw_size_of(
            half(tex.size_vec2()),
            half(self.original_size(tex)),
            container,
            self.zoom_factor,
        );
        let visible = Rect::from_center_size(center, half_size);
        let full_min_x = if shows_right {
            visible.min.x - half_size.x
        } else {
            visible.min.x
        };
        let full = Rect::from_min_size(
            egui::pos2(full_min_x, visible.min.y),
            egui::vec2(half_size.x * 2.0, half_size.y),
        );
        Some((full, visible))
    }

    /// 元画像の大きさ。テクスチャは表示用に縮めてあることがあるので、読み込み時の記録を使う
    fn original_size(&self, tex: &egui::TextureHandle) -> egui::Vec2 {
        self.image_dimensions.get(&tex.name()).map_or_else(
            || tex.size_vec2(),
            |&[width, height]| egui::vec2(width as f32, height as f32),
        )
    }

    /// 見開きの2枚（textures[0], textures[1]）の描画サイズ。`half` は1枚ぶんの領域。
    /// 「縦サイズを揃える」では2枚を同じ高さにする
    pub(super) fn spread_draw_sizes(&self, half: egui::Vec2, zoom_factor: f32) -> [egui::Vec2; 2] {
        if self.config.image_sizing_mode == ImageSizingMode::MatchHeight
            && let [Some(first), Some(second)] = &self.textures
        {
            let aspect = |tex: &egui::TextureHandle| tex.size_vec2().x / tex.size_vec2().y;
            let aspects = [aspect(first), aspect(second)];
            let height = matched_height(aspects, half.into()) * zoom_factor;
            return aspects.map(|aspect| egui::vec2(height * aspect, height));
        }
        [0, 1].map(|index| {
            self.textures[index]
                .as_ref()
                .map_or(egui::Vec2::ZERO, |tex| {
                    self.image_draw_size(tex, half, zoom_factor)
                })
        })
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
            ImageSizingMode::FitBoth | ImageSizingMode::MatchHeight => {
                let scale = (container_size.x.max(1.0) / tex_size.x)
                    .min(container_size.y.max(1.0) / tex_size.y);
                tex_size * scale * self.zoom_factor
            }
            ImageSizingMode::ShrinkOnly => {
                let original = self.original_size(tex);
                let container = [container_size.x.max(1.0), container_size.y.max(1.0)];
                original * shrink_only_scale(original.into(), container) * self.zoom_factor
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

    /// 1枚目を単独で中央に見せている状態か（奇数ページ開始で、白紙を置かない設定）
    pub(super) fn cover_shown_alone(&self) -> bool {
        self.is_shifted && self.current_index == 0 && !self.config.cover_blank_page
    }

    /// 1枚目を白紙と並べて見開きで見せる状態か（先頭ページ挿入。MM-VIEW-07）
    pub(super) fn cover_paired_with_blank(&self) -> bool {
        self.config.cover_blank_page
            && self.is_shifted
            && self.current_index == 0
            && !self.is_single_page()
    }

    /// 最後の1枚を白紙と並べて見開きで見せる状態か（MM-NAV-01）。ミーヤと同じく、
    /// 2ページ表示ではいつも、自動見開きでは先頭ページ挿入のときだけ（観察リスト4, 5）。
    /// 1枚目を単独で中央に見せる設定のときは、最後の1枚も白紙と並べない
    pub(super) fn last_page_paired_with_blank(&self) -> bool {
        if self.is_single_page()
            || self.image_files.len().checked_sub(1) != Some(self.current_index)
            || (self.is_shifted && self.current_index == 0)
        {
            return false;
        }
        let inserting_blank = self.is_shifted && self.config.cover_blank_page;
        if self.automatic_page_direction().is_some() {
            inserting_blank
        } else {
            inserting_blank || !self.is_shifted
        }
    }

    /// 先頭や最後に並べた白紙か。白紙は作るたびに別のテクスチャになるので、名前で見分ける
    pub(super) fn is_cover_blank(&self, texture: &Option<egui::TextureHandle>) -> bool {
        texture
            .as_ref()
            .is_some_and(|texture| texture.name() == BLANK_TEXTURE_NAME)
    }

    /// `page` と同じ縦横比の白紙（色が一様なので小さくてよい）
    fn blank_like(&self, page: &egui::TextureHandle, ctx: &egui::Context) -> egui::TextureHandle {
        let [width, height] = page.size();
        let scale = COVER_BLANK_MAX_SIDE / width.max(height).max(1) as f32;
        let size = [
            ((width as f32 * scale).round() as usize).max(1),
            ((height as f32 * scale).round() as usize).max(1),
        ];
        ctx.load_texture(
            BLANK_TEXTURE_NAME,
            egui::ColorImage::new(size, vec![egui::Color32::WHITE; size[0] * size[1]]),
            egui::TextureOptions::LINEAR,
        )
    }

    /// 表示中の2枚を、先頭ページ挿入と最後の1枚の白紙の状態に合わせる。
    /// 白紙はどちらも読む順で来る側に置く: 先頭は「先に読む側」（右開きなら右）に置いて1枚目をその隣に移し、
    /// 最後は「後に読む側」（右開きなら左）に置く
    pub(super) fn sync_cover_blank(&mut self, ctx: &egui::Context) {
        let want_trailing = self.last_page_paired_with_blank();
        let has_trailing = self.is_cover_blank(&self.textures[1]);
        if has_trailing && !want_trailing {
            self.textures[1] = None;
        } else if want_trailing
            && !has_trailing
            && self.textures[1].is_none()
            && let Some(page) = self.textures[0].clone()
        {
            self.textures[1] = Some(self.blank_like(&page, ctx));
        }

        let want_blank = self.cover_paired_with_blank();
        let has_blank = self.is_cover_blank(&self.textures[0]);
        if has_blank && want_blank && self.textures[1].is_some() {
            return;
        }
        if has_blank {
            // いったん白紙を外して、1枚目を元の位置に戻す
            self.textures = [self.textures[1].take(), None];
            if self.textures[0].is_none() && self.current_index < self.image_files.len() {
                // 白紙だけが残っていた（1枚目が失われていた）ときは読み直す
                self.textures = self.load_pair(self.current_index, ctx);
                self.apply_automatic_page_layout();
            }
        }
        if want_blank {
            let Some(cover) = self.textures[0].take() else {
                return;
            };
            // 白紙は1枚目と同じ縦横比にする
            let blank = self.blank_like(&cover, ctx);
            self.textures = [Some(blank), Some(cover)];
        }
    }

    /// 今のページを pan_offset = 0 で描いたときに占める範囲（render_default_page_view と同じ計算）。
    /// ページが無いときは None
    pub(super) fn page_content_rect(&self, rect: Rect) -> Option<Rect> {
        let viewing_single = self.is_single_page() || self.cover_shown_alone();
        if viewing_single {
            return self
                .single_page_rects(rect.center(), rect.size())
                .map(|(_, visible)| visible);
        }
        let offset = self.config.spread_center_offset;
        let container = egui::vec2((rect.width() * 0.5 - offset.abs()).max(1.0), rect.height());
        if self.textures.iter().all(Option::is_none) {
            return None;
        }
        let [first, second] = self.spread_draw_sizes(container, self.zoom_factor);
        // 右開きでは textures[0] が右、左開きでは左に来る
        let (left, right) = if self.config.page_view_options == PageViewOptions::DoubleLR {
            (first, second)
        } else {
            (second, first)
        };
        let height = left.y.max(right.y);
        let center = rect.center();
        Some(Rect::from_min_max(
            egui::pos2(center.x - offset - left.x, center.y - height * 0.5),
            egui::pos2(center.x + offset + right.x, center.y + height * 0.5),
        ))
    }

    /// 表示位置をずらせる範囲（横, 縦）
    pub(super) fn scroll_spans(&self) -> Option<(Span, Span)> {
        let rect = self.last_page_view_rect?;
        let content = self.page_content_rect(rect)?;
        Some((
            Span::new(content.min.x, content.max.x, rect.min.x, rect.max.x),
            Span::new(content.min.y, content.max.y, rect.min.y, rect.max.y),
        ))
    }

    pub(super) fn double_page_needs_drag(&self, rect: Rect) -> bool {
        let page_size = egui::vec2(rect.width() * 0.5, rect.height());
        self.spread_draw_sizes(page_size, self.zoom_factor)
            .iter()
            .any(|size| size.x > page_size.x || size.y > page_size.y)
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
                if self.config.image_sizing_mode.centers_vertically() {
                    let image_x = match align {
                        egui::Align::RIGHT => rect.right() - image_size.x,
                        egui::Align::Center => rect.center().x - image_size.x * 0.5,
                        egui::Align::LEFT => rect.left(),
                    };
                    let image_y = rect.center().y - image_size.y * 0.5;
                    paint_page(
                        ui,
                        &mut self.drawn_pages,
                        tex.id(),
                        egui::Rect::from_min_size(egui::pos2(image_x, image_y), image_size),
                        ui.clip_rect(),
                    );
                } else {
                    let layout = egui::Layout::top_down(align);
                    ui.with_layout(layout, |ui| {
                        let image = ui.add(
                            egui::Image::new(tex)
                                .fit_to_exact_size(image_size)
                                .maintain_aspect_ratio(true),
                        );
                        self.drawn_pages.push(DrawnPage {
                            texture: tex.id(),
                            rect: image.rect,
                            clip: ui.clip_rect(),
                        });
                    });
                }
                ui.set_clip_rect(previous_clip_rect);
            }
            resp
        })
        .inner
    }

    /// ルーペを出し入れする。拡大して見るためにページを高い解像度で読み直す
    pub(super) fn set_loupe_enabled(&mut self, enabled: bool, ctx: &egui::Context) {
        if self.loupe_enabled == enabled {
            return;
        }
        self.loupe_enabled = enabled;
        if self.zip_path.is_some() && !self.image_files.is_empty() {
            self.reload_zoom_textures(ctx);
        }
        ctx.request_repaint();
    }

    /// ルーペの描画（MM-LOUPE-01）: カーソルの周りの円に、このフレームで描いたページを拡大して重ねる
    pub(super) fn render_loupe(&self, ctx: &egui::Context) {
        if !self.loupe_enabled || self.drawn_pages.is_empty() {
            return;
        }
        let (Some(pointer), Some(view)) = (ctx.pointer_hover_pos(), self.last_page_view_rect)
        else {
            return;
        };
        if !view.contains(pointer) {
            return;
        }
        let radius = self.config.loupe_radius;
        let magnification = self.config.loupe_magnification;
        let center = [pointer.x, pointer.y];
        let painter = ctx
            .layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("loupe"),
            ))
            .with_clip_rect(view);
        let circle = loupe::circle(center, radius, LOUPE_SEGMENTS);
        let to_pos = |p: loupe::Point| egui::pos2(p[0], p[1]);

        // ページの外側が映る部分は背景色にする
        painter.add(egui::Shape::convex_polygon(
            circle.iter().copied().map(to_pos).collect(),
            self.image_panel_background_color(),
            egui::Stroke::NONE,
        ));
        for page in &self.drawn_pages {
            // ページの見えている範囲がルーペの中で占める範囲で、円を切り抜く
            let visible = page.rect.intersect(page.clip);
            if !visible.is_positive() {
                continue;
            }
            let min = loupe::loupe_point(center, [visible.min.x, visible.min.y], magnification);
            let max = loupe::loupe_point(center, [visible.max.x, visible.max.y], magnification);
            let polygon = loupe::clip_to_rect(&circle, min, max);
            if polygon.len() < 3 {
                continue;
            }
            let mut mesh = egui::Mesh::with_texture(page.texture);
            for point in &polygon {
                let source = loupe::source_point(center, *point, magnification);
                let uv = egui::pos2(
                    (source[0] - page.rect.min.x) / page.rect.width(),
                    (source[1] - page.rect.min.y) / page.rect.height(),
                );
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: to_pos(*point),
                    uv,
                    color: egui::Color32::WHITE,
                });
            }
            // 凸多角形なので扇形に三角形を張る
            for i in 1..polygon.len() as u32 - 1 {
                mesh.add_triangle(0, i, i + 1);
            }
            painter.add(egui::Shape::mesh(mesh));
        }
        painter.circle_stroke(
            pointer,
            radius,
            egui::Stroke::new(2.0_f32, egui::Color32::from_gray(160)),
        );
    }
}

/// 描いたページの記録（ルーペ用）
pub(super) struct DrawnPage {
    pub texture: egui::TextureId,
    /// 画像全体を描いた範囲（はみ出している部分も含む）
    pub rect: Rect,
    /// 実際に見えている範囲
    pub clip: Rect,
}

/// ページの画像を描き、ルーペ用に記録する
pub(super) fn paint_page(
    ui: &egui::Ui,
    drawn: &mut Vec<DrawnPage>,
    texture: egui::TextureId,
    image_rect: Rect,
    visible_rect: Rect,
) {
    let clip = ui.clip_rect().intersect(visible_rect);
    ui.painter().with_clip_rect(clip).image(
        texture,
        image_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    drawn.push(DrawnPage {
        texture,
        rect: image_rect,
        clip,
    });
}
