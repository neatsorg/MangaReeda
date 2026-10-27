use super::{AUTO_SCROLL_MAX_DELAY_MS, AUTO_SCROLL_MIN_DELAY_MS, MangaReader, SCROLL_STEP_RATIO};
use crate::config::{LastPageAction, MangaAction, PageViewOptions};
use crate::engine::layout::{automatic_pair_count, is_portrait};
use crate::engine::library::{get_adjacent_directories, scan_folder};
use crate::engine::scroll;
use crate::localize::tr;
use eframe::egui;
use std::time::{Duration, Instant};

impl MangaReader {
    pub(super) fn auto_scroll_delay_ms(&self) -> u64 {
        self.config
            .image_delay
            .clamp(AUTO_SCROLL_MIN_DELAY_MS, AUTO_SCROLL_MAX_DELAY_MS)
    }

    pub(super) fn schedule_auto_scroll(&mut self, ctx: &egui::Context) {
        let delay = Duration::from_millis(self.auto_scroll_delay_ms());
        let next_tick = Instant::now() + delay;
        self.auto_scroll_next_tick = Some(next_tick);
        ctx.request_repaint_after(delay);
    }

    pub(super) fn stop_auto_scroll(&mut self) {
        self.auto_scroll_enabled = false;
        self.auto_scroll_next_tick = None;
    }

    pub(super) fn set_auto_scroll_enabled(&mut self, enabled: bool, ctx: &egui::Context) {
        if enabled {
            self.auto_scroll_enabled = true;
            self.schedule_auto_scroll(ctx);
        } else {
            self.stop_auto_scroll();
        }
    }

    pub(super) fn toggle_auto_scroll(&mut self, ctx: &egui::Context) {
        let enabled = !self.auto_scroll_enabled;
        self.set_auto_scroll_enabled(enabled, ctx);
    }

    pub(super) fn automatic_page_direction(&self) -> Option<PageViewOptions> {
        if self.config.page_view_options == PageViewOptions::TopDown {
            None
        } else if self.config.auto_page_layout_rl {
            Some(PageViewOptions::DoubleRL)
        } else if self.config.auto_page_layout_lr {
            Some(PageViewOptions::DoubleLR)
        } else {
            None
        }
    }

    pub(super) fn page_dimensions(&self, index: usize) -> Option<[u32; 2]> {
        self.image_dimensions
            .get(self.image_files.get(index)?)
            .copied()
    }

    pub(super) fn automatic_page_count(&self, index: usize) -> usize {
        automatic_pair_count(
            self.page_dimensions(index),
            self.page_dimensions(index + 1),
            self.is_shifted && index == 0,
        )
    }

    pub(super) fn apply_automatic_page_layout(&mut self) {
        let Some(direction) = self.automatic_page_direction() else {
            return;
        };
        let Some(size) = self.page_dimensions(self.current_index) else {
            return;
        };
        self.config.page_view_options = if is_portrait(size) {
            direction
        } else {
            PageViewOptions::Single
        };
        if self.automatic_page_count(self.current_index) == 1 {
            // 先頭ページ挿入の白紙が入っていると、スロット1は1枚目なので、先に白紙を外す
            if self.is_cover_blank(&self.textures[0]) {
                self.textures = [self.textures[1].take(), None];
            }
            self.textures[1] = None;
            self.active_animations[1] = None;
            self.active_animation_keys[1] = None;
        }
    }

    /// 固定の見開き（自動見開きでない）では、見開きの組の先頭に位置を合わせる。
    /// 1ページ表示から切り替えたときなどに、組が1枚ずれたままになるのを防ぐ
    fn align_to_spread_start(&mut self) {
        if self.is_single_page() || self.automatic_page_direction().is_some() {
            return;
        }
        let index = self.current_index;
        self.current_index = if self.is_shifted {
            // 1枚目だけ単独で、以降は (1,2), (3,4)…
            if index == 0 || index % 2 == 1 {
                index
            } else {
                index - 1
            }
        } else {
            index - index % 2
        };
    }

    pub(super) fn load_current_pair(&mut self, ctx: &egui::Context) {
        self.align_to_spread_start();
        self.textures = self.load_pair(self.current_index, ctx);
        self.apply_automatic_page_layout();
        self.sync_cover_blank(ctx);
    }

    pub(super) fn automatic_previous_index(&mut self, ctx: &egui::Context) -> usize {
        let previous = self.current_index.saturating_sub(1);
        if self.page_dimensions(previous).is_none() {
            let _ = self.load_pair(previous, ctx);
        }
        if !self.page_dimensions(previous).is_some_and(is_portrait) {
            return previous;
        }
        // Align pairs within each portrait run. An odd final portrait before a
        // landscape page must remain a singleton when navigating backward too.
        let mut run_start = previous;
        while run_start > 0 && !(self.is_shifted && run_start == 1) {
            let candidate = run_start - 1;
            if self.page_dimensions(candidate).is_none() {
                let _ = self.load_pair(candidate, ctx);
            }
            if !self.page_dimensions(candidate).is_some_and(is_portrait) {
                break;
            }
            run_start = candidate;
        }
        run_start + ((previous - run_start) / 2) * 2
    }

    pub(super) fn update_buffers(&mut self, ctx: &egui::Context) {
        let idx = self.current_index;

        if self.is_scrubbing {
            return;
        }

        if self.zip_path.is_none() {
            return;
        }

        if self.zoom_factor != 1.0 {
            return;
        }

        // Only update if we moved OR if the buffers were recently consumed
        if self.last_buffered_index == Some(idx) {
            return;
        }

        if self.automatic_page_direction().is_some() {
            let next_index = idx + self.automatic_page_count(idx);
            self.buffer_next = self.load_pair(next_index, ctx);
            if idx > 0 {
                let previous_index = self.automatic_previous_index(ctx);
                self.buffer_prev = self.load_pair(previous_index, ctx);
            }
            self.last_buffered_index = Some(idx);
            return;
        }

        let step = if self.is_single_page() { 1 } else { 2 };

        // Preload Next (2 pages ahead)
        if self.buffer_next[0].is_none() {
            let mut next_index_to_load = idx + step;
            if idx == 0 && self.is_shifted {
                next_index_to_load = 1;
            }
            self.buffer_next = self.load_pair(next_index_to_load, ctx);
        }

        // Preload Prev (2 pages behind)
        if idx >= step && self.buffer_prev[0].is_none() {
            self.buffer_prev = self.load_pair(idx - step, ctx);
        }

        // Preload prev buffer case for when using cover mode
        if self.is_shifted && idx >= 1 && self.buffer_prev[0].is_none() {
            self.buffer_prev = self.load_pair(0, ctx);
        }

        self.last_buffered_index = Some(idx);
    }

    pub(super) fn next_page(&mut self, ctx: &egui::Context, force_step_one: bool) {
        if self.last_image_switch_time + Duration::from_millis(self.config.image_delay)
            > Instant::now()
        {
            return;
        } else {
            self.last_image_switch_time = Instant::now();
        }
        let step = if force_step_one {
            // reset the buffer when force step 1 to prevent 2 images copy from buffer to active
            self.buffer_next = [None, None];
            self.buffer_prev = [None, None];
            1
        } else if self.automatic_page_direction().is_some() {
            self.automatic_page_count(self.current_index)
        } else if self.is_single_page() || (self.is_shifted && self.current_index == 0) {
            1
        } else {
            2
        };

        if self.current_index + step < self.image_files.len() {
            self.current_index += step;
            self.reset_pan();
            self.top_down_scroll_offset = 0.0;
            if self.config.page_view_options == PageViewOptions::TopDown {
                self.ensure_top_down_loaded_around(self.current_index, ctx);
            } else if self.buffer_next[0].is_some() {
                // If the next pages are already in the buffer, swap them in
                // Take the textures from the buffer and put them in the active slot
                // also slide the current texture to prev buffer
                self.buffer_prev = std::mem::take(&mut self.textures);
                self.textures = std::mem::take(&mut self.buffer_next);
            } else {
                // Fallback if buffer wasn't ready (e.g., very fast clicking)
                // 前のページの先読みは古いページのままなので捨てる（残すと「前へ」で古いページが出る）
                self.buffer_prev = [None, None];
                self.load_current_pair(ctx);
            }
            self.buffer_next = [None, None];
        } else {
            // End of Zip list reached, do the last page action
            match self.config.last_page_action {
                LastPageAction::GotoNextFile => self.next_zip(ctx),
                LastPageAction::ToFirstPage => self.go_to_first_page(ctx),
                LastPageAction::Nothing => {
                    let msg = tr("error.no_more_pages").to_owned();
                    self.show_fading_error(&msg);
                }
            }
        }
        if self.automatic_page_direction().is_some() {
            self.apply_automatic_page_layout();
            self.reset_buffer();
        }
        self.sync_cover_blank(ctx);
        self.page_indicator_time = Some(Instant::now());
    }

    pub(super) fn prev_page(&mut self, ctx: &egui::Context, force_step_one: bool) {
        if self.last_image_switch_time + Duration::from_millis(self.config.image_delay)
            > Instant::now()
        {
            return;
        } else {
            self.last_image_switch_time = Instant::now();
        }
        let step = if force_step_one {
            // reset the buffer when force step 1 to prevent 2 images copy from buffer to active
            self.buffer_next = [None, None];
            self.buffer_prev = [None, None];
            1
        } else if self.automatic_page_direction().is_some() {
            self.current_index - self.automatic_previous_index(ctx)
        } else if self.is_single_page() || (self.is_shifted && self.current_index == 1) {
            1
        } else {
            2
        };

        if self.current_index > 0 && self.current_index >= step {
            self.current_index -= step;
            self.reset_pan();
            self.top_down_scroll_offset = 0.0;
            if self.config.page_view_options == PageViewOptions::TopDown {
                self.ensure_top_down_loaded_around(self.current_index, ctx);
            } else if self.buffer_prev[0].is_some() {
                // Use the previous buffer textures
                self.buffer_next = std::mem::take(&mut self.textures);
                self.textures = std::mem::take(&mut self.buffer_prev);
            } else {
                // 次のページの先読みは古いページのままなので捨てる
                self.buffer_next = [None, None];
                self.load_current_pair(ctx);
            }
            self.buffer_prev = [None, None];
        } else {
            // we are at the start of the Zip, do the last page action
            match self.config.last_page_action {
                LastPageAction::GotoNextFile => self.prev_zip(ctx, true),
                LastPageAction::ToFirstPage => self.go_to_last_page(ctx),
                LastPageAction::Nothing => {
                    let msg = tr("error.first_page").to_owned();
                    self.show_fading_error(&msg)
                }
            }
        }
        if self.automatic_page_direction().is_some() {
            self.apply_automatic_page_layout();
            self.reset_buffer();
        }
        self.sync_cover_blank(ctx);
        self.page_indicator_time = Some(Instant::now());
    }

    pub(super) fn next_zip(&mut self, ctx: &egui::Context) {
        if let Some(pos) = self
            .all_zips_in_folder
            .iter()
            .position(|p| Some(p) == self.zip_path.as_ref())
        {
            if pos + 1 < self.all_zips_in_folder.len() {
                // There is a next file
                let next_path = self.all_zips_in_folder[pos + 1].clone();
                self.load_source(next_path, ctx);
            } else {
                // NO MORE FILES - This is the fix
                let msg = tr("error.no_more_zip_files").to_owned();
                self.show_fading_error(&msg);
            }
        }
    }

    /// 前の冊を開く。`open_at_last_page` が真なら最終ページから表示する
    /// （先頭ページから戻ったときの挙動。MM-NAV-22）
    pub(super) fn prev_zip(&mut self, ctx: &egui::Context, open_at_last_page: bool) {
        if let Some(pos) = self
            .all_zips_in_folder
            .iter()
            .position(|p| Some(p) == self.zip_path.as_ref())
        {
            if pos > 0 {
                let prev_path = self.all_zips_in_folder[pos - 1].clone();
                self.load_source(prev_path.clone(), ctx);
                // 読み込みに失敗したときは元の冊のままなので、ページを動かさない
                if open_at_last_page && self.zip_path.as_ref() == Some(&prev_path) {
                    self.go_to_last_page(ctx);
                }
            } else {
                let msg = tr("error.no_previous_zip_files").to_owned();
                self.show_fading_error(&msg);
            }
        }
    }

    pub(super) fn next_folder(&mut self, ctx: &egui::Context) {
        let (_, next_dir) = get_adjacent_directories(self.zip_path.clone());

        // Check if next_dir actually exists
        if let Some(dir) = next_dir {
            let zips = scan_folder(&*dir);
            if zips.is_empty() {
                let msg = tr("error.no_archive_next_folder").to_owned();
                self.show_fading_error(&msg);
            } else {
                self.load_source(zips[0].clone(), ctx);
            }
        } else {
            let msg = tr("error.no_next_directory").to_owned();
            self.show_fading_error(&msg);
        }
    }

    pub(super) fn prev_folder(&mut self, ctx: &egui::Context) {
        let (prev_dir, _) = get_adjacent_directories(self.zip_path.clone());

        // Check if next_dir actually exists
        if let Some(dir) = prev_dir {
            let zips = scan_folder(&*dir);
            if zips.is_empty() {
                let msg = tr("error.no_archive_prev_folder").to_owned();
                self.show_fading_error(&msg);
            } else {
                self.load_source(zips[0].clone(), ctx);
            }
        } else {
            let msg = tr("error.no_previous_directory").to_owned();
            self.show_fading_error(&msg);
        }
    }

    pub(super) fn go_to_first_page(&mut self, ctx: &egui::Context) {
        if !self.image_files.is_empty() && self.current_index != 0 {
            self.reset_buffer();
            self.reset_pan();
            self.current_index = 0;
            self.load_current_pair(ctx);
            self.page_indicator_time = Some(Instant::now());
        }
    }

    pub(super) fn go_to_page(&mut self, index: usize, ctx: &egui::Context) {
        if index < self.image_files.len() && self.current_index != index {
            self.reset_buffer();
            self.reset_pan();
            self.current_index = index;
            self.load_current_pair(ctx);
            self.page_indicator_time = Some(Instant::now());
        }
    }

    pub(super) fn go_to_last_page(&mut self, ctx: &egui::Context) {
        if !self.image_files.is_empty() {
            // Find the start of the final view using the active layout's grouping.
            let last_idx = if self.automatic_page_direction().is_some() {
                // Treat the position just past the end as a backward page turn.
                let current_index = self.current_index;
                self.current_index = self.image_files.len();
                let last_idx = self.automatic_previous_index(ctx);
                self.current_index = current_index;
                last_idx
            } else {
                let last = self.image_files.len() - 1;
                if self.is_single_page() {
                    last
                } else if self.is_shifted {
                    // 1枚目だけ単独で、以降は (1,2), (3,4)… と組む
                    if last == 0 {
                        0
                    } else {
                        ((last - 1) / 2) * 2 + 1
                    }
                } else {
                    (last / 2) * 2
                }
            };
            if self.current_index != last_idx {
                self.reset_buffer();
                self.reset_pan();
                self.current_index = last_idx;
                self.load_current_pair(ctx);
                self.page_indicator_time = Some(Instant::now());
            }
        }
    }

    /// リピート状態（冊の端に来たときの動作）を順に切り替えて保存する（MM-NAV-20）
    pub(super) fn cycle_last_page_action(&mut self) {
        self.config.last_page_action = self.config.last_page_action.next_in_cycle();
        self.save_settings();
        let msg = match self.config.last_page_action {
            LastPageAction::Nothing => tr("mode.repeat_off"),
            LastPageAction::ToFirstPage => tr("mode.repeat_on"),
            LastPageAction::GotoNextFile => tr("mode.repeat_folder"),
        };
        self.show_fading_error(msg);
    }

    /// 右開きで読んでいるか。1ページ表示のときは自動見開きの向きに従い、決まっていなければ右開き
    pub(super) fn reads_right_to_left(&self) -> bool {
        match self.config.page_view_options {
            PageViewOptions::DoubleLR => false,
            PageViewOptions::DoubleRL => true,
            _ => !self.config.auto_page_layout_lr || self.config.auto_page_layout_rl,
        }
    }

    fn scroll_step(&self) -> scroll::Position {
        let size = self
            .last_page_view_rect
            .map_or(egui::Vec2::ZERO, |rect| rect.size() * SCROLL_STEP_RATIO);
        (size.x, size.y)
    }

    fn set_pan(&mut self, (x, y): scroll::Position, ctx: &egui::Context) {
        self.pan_offset = egui::vec2(x, y);
        self.pan_to_start_pending = false;
        ctx.request_repaint();
    }

    /// 上下左右へ1回分スクロールする（MM-NAV-09）。端より先へは動かない。
    /// `dx`・`dy` は見たい方向（右・下が正）
    pub(super) fn scroll_view(&mut self, dx: f32, dy: f32, ctx: &egui::Context) {
        if self.config.page_view_options == PageViewOptions::TopDown {
            match dy {
                dy if dy > 0.0 => self.execute_action(MangaAction::SlideImageDown, ctx),
                dy if dy < 0.0 => self.execute_action(MangaAction::SlideImageUp, ctx),
                _ => {}
            }
            return;
        }
        let Some((x, y)) = self.scroll_spans() else {
            return;
        };
        let step = self.scroll_step();
        // 下を見るには中身を上へずらすので、見たい方向と逆に動かす
        let target = (
            x.clamp(self.pan_offset.x - dx * step.0),
            y.clamp(self.pan_offset.y - dy * step.1),
        );
        self.set_pan(target, ctx);
    }

    /// スクロール兼次へ（MM-NAV-08）: はみ出していれば読む順にスクロールし、
    /// 読み終わりまで来ていたら次のページを読み始めの位置から表示する
    pub(super) fn scroll_next_page(&mut self, ctx: &egui::Context) {
        if self.config.page_view_options == PageViewOptions::TopDown {
            self.execute_action(MangaAction::SlideImageDown, ctx);
            return;
        }
        let rtl = self.reads_right_to_left();
        if let Some((x, y)) = self.scroll_spans() {
            let position = (self.pan_offset.x, self.pan_offset.y);
            if let Some(next) = scroll::step_forward(position, x, y, self.scroll_step(), rtl) {
                self.set_pan(next, ctx);
                return;
            }
        }
        let before = (self.zip_path.clone(), self.current_index);
        self.next_page(ctx, false);
        if before != (self.zip_path.clone(), self.current_index) {
            if let Some((x, y)) = self.scroll_spans() {
                self.set_pan(scroll::start_position(x, y, rtl), ctx);
            }
        }
    }

    /// スクロール兼前へ: scroll_next_page の逆。前のページは読み終わりの位置から表示する
    pub(super) fn scroll_prev_page(&mut self, ctx: &egui::Context) {
        if self.config.page_view_options == PageViewOptions::TopDown {
            self.execute_action(MangaAction::SlideImageUp, ctx);
            return;
        }
        let rtl = self.reads_right_to_left();
        if let Some((x, y)) = self.scroll_spans() {
            let position = (self.pan_offset.x, self.pan_offset.y);
            if let Some(previous) = scroll::step_backward(position, x, y, self.scroll_step(), rtl) {
                self.set_pan(previous, ctx);
                return;
            }
        }
        let before = (self.zip_path.clone(), self.current_index);
        self.prev_page(ctx, false);
        if before != (self.zip_path.clone(), self.current_index) {
            if let Some((x, y)) = self.scroll_spans() {
                self.set_pan(scroll::end_position(x, y, rtl), ctx);
            }
        }
    }

    pub(super) fn reset_buffer(&mut self) {
        self.last_buffered_index = None;
        self.buffer_prev = [None, None];
        self.buffer_next = [None, None];
        self.top_down_textures.clear();
        self.top_down_scroll_offset = 0.0;
        self.clear_active_animations();
    }

    pub(super) fn is_single_page(&self) -> bool {
        matches!(
            self.config.page_view_options,
            PageViewOptions::Single | PageViewOptions::TopDown
        )
    }

    pub(super) fn change_shifted_mode(&mut self, ctx: &egui::Context) {
        self.is_shifted = !self.is_shifted;

        // Adjust current_index to keep the view consistent
        if self.is_shifted {
            if self.current_index == 0 {
                self.current_index = 0;
            } else if self.current_index % 2 == 0 {
                self.current_index += 1;
            }
        } else {
            // Return to even index alignment
            self.current_index = self.current_index.saturating_sub(1);
            if self.current_index % 2 != 0 {
                self.current_index = self.current_index.saturating_sub(1);
            }
        }

        self.reset_buffer();
        self.texture_cache.clear();
        self.reset_pan();
        self.load_current_pair(ctx);
        let msg = if self.is_shifted {
            tr("mode.odd_page")
        } else {
            tr("mode.even_page")
        };
        self.show_fading_error(msg);
    }

    pub(super) fn tick_auto_scroll(&mut self, ctx: &egui::Context, action_to_run: MangaAction) {
        if self.auto_scroll_enabled {
            if self.image_files.is_empty() {
                self.stop_auto_scroll();
            } else if action_to_run == MangaAction::None {
                if let Some(next_tick) = self.auto_scroll_next_tick {
                    let now = Instant::now();
                    if now >= next_tick {
                        let before_path = self.zip_path.clone();
                        let before_index = self.current_index;
                        self.next_page(ctx, false);
                        let page_advanced =
                            self.current_index != before_index || self.zip_path != before_path;
                        if page_advanced {
                            self.schedule_auto_scroll(ctx);
                        } else {
                            self.stop_auto_scroll();
                        }
                    } else {
                        ctx.request_repaint_after(next_tick - now);
                    }
                } else {
                    self.schedule_auto_scroll(ctx);
                }
            } else {
                self.schedule_auto_scroll(ctx);
            }
        }
    }
}
