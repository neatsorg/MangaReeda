use super::{
    AUTO_SCROLL_MAX_DELAY_MS, AUTO_SCROLL_MIN_DELAY_MS, FULLSCREEN_TITLE_BAR_HEIGHT,
    FULLSCREEN_TOP_CHROME_REVEAL_HEIGHT, MangaReader,
};
use crate::localize::tr;
use eframe::egui;

/// 履歴メニューに出す冊の数
const HISTORY_MENU_ITEMS: usize = 20;

impl MangaReader {
    pub(super) fn set_fullscreen(&mut self, fullscreen: bool, ctx: &egui::Context) {
        self.is_fullscreen = fullscreen;
        if !fullscreen {
            self.fullscreen_top_chrome_visible = false;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
    }

    pub(super) fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.set_fullscreen(!self.is_fullscreen, ctx);
    }

    pub(super) fn update_fullscreen_top_chrome_visibility(&mut self, ctx: &egui::Context) {
        if !self.is_fullscreen {
            self.fullscreen_top_chrome_visible = false;
            return;
        }

        let toolbar_scale = self.config.top_toolbar_scale.clamp(1.0, 3.0);
        let visible_hover_height = FULLSCREEN_TITLE_BAR_HEIGHT + (42.0 * toolbar_scale);
        let hover_height = if self.fullscreen_top_chrome_visible {
            visible_hover_height
        } else {
            FULLSCREEN_TOP_CHROME_REVEAL_HEIGHT
        };
        let should_show = ctx
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|pos| pos.y <= hover_height);

        if self.fullscreen_top_chrome_visible != should_show {
            self.fullscreen_top_chrome_visible = should_show;
            ctx.request_repaint();
        }
    }

    pub(super) fn render_fullscreen_title_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("fullscreen_title_bar")
            .exact_height(FULLSCREEN_TITLE_BAR_HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_gray(24))
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let title = tr("app.title");
                    let title_width = (ui.available_width() - 92.0).max(0.0);
                    let title_response = ui.add_sized(
                        [title_width, ui.available_height()],
                        egui::Label::new(
                            egui::RichText::new(title)
                                .color(egui::Color32::from_gray(220))
                                .strong(),
                        )
                        .sense(egui::Sense::drag()),
                    );
                    if title_response.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }

                    if ui.button("—").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    if ui
                        .button("🗗")
                        .on_hover_text(tr("toolbar.fullscreen"))
                        .clicked()
                    {
                        self.set_fullscreen(false, ctx);
                    }
                    if ui.button("×").clicked() {
                        self.save_settings();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
    }

    /// 履歴から冊を選んで、しおりのページから開くメニュー
    fn render_history_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut chosen = None;
        ui.menu_button("🕘", |ui| {
            if self.history.entries.is_empty() {
                ui.label(tr("toolbar.history.empty"));
            }
            for entry in self.history.entries.iter().take(HISTORY_MENU_ITEMS) {
                let name = entry
                    .path
                    .file_name()
                    .map_or_else(|| entry.path.to_string_lossy(), |n| n.to_string_lossy());
                let label = format!("{name}  ({}/{})", entry.page_index + 1, entry.page_count);
                if ui
                    .button(label)
                    .on_hover_text(entry.path.to_string_lossy())
                    .clicked()
                {
                    chosen = Some(entry.path.clone());
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(tr("toolbar.history"));
        if let Some(path) = chosen {
            if path.exists() {
                self.open_source(path, ctx);
            } else {
                self.show_fading_error(tr("error.history_missing"));
            }
        }
    }

    pub(super) fn render_top_toolbar(&mut self, ctx: &egui::Context) {
        self.update_fullscreen_top_chrome_visibility(ctx);
        let show_top_toolbar =
            self.config.show_top_bar && (!self.is_fullscreen || self.fullscreen_top_chrome_visible);

        if self.is_fullscreen && self.fullscreen_top_chrome_visible {
            self.render_fullscreen_title_bar(ctx);
        }

        if show_top_toolbar {
            egui::TopBottomPanel::top("top_toolbar")
                .frame(
                    egui::Frame::NONE
                        .fill(egui::Color32::from_gray(30))
                        .inner_margin(4.0),
                )
                .show(ctx, |ui| {
                    ui.scope(|ui| {
                        let toolbar_scale = self.config.top_toolbar_scale.clamp(1.0, 3.0);
                        let style = ui.style_mut();
                        style.spacing.interact_size.y *= toolbar_scale;
                        style.spacing.button_padding *= toolbar_scale;
                        style.spacing.item_spacing *= toolbar_scale;
                        if let Some(font_id) = style.text_styles.get_mut(&egui::TextStyle::Button) {
                            font_id.size *= toolbar_scale;
                        }
                        if let Some(font_id) = style.text_styles.get_mut(&egui::TextStyle::Body) {
                            font_id.size *= toolbar_scale;
                        }

                        ui.horizontal(|ui| {
                            // --- Folder Navigation ---
                            if ui
                                .button("📁⏮")
                                .on_hover_text(tr("toolbar.prev_folder"))
                                .clicked()
                            {
                                self.prev_folder(ctx);
                            }
                            if ui
                                .button("⏭📁")
                                .on_hover_text(tr("toolbar.next_folder"))
                                .clicked()
                            {
                                self.next_folder(ctx);
                            }
                            ui.separator();

                            // --- File Navigation ---
                            if ui
                                .button("📦⏮")
                                .on_hover_text(tr("toolbar.prev_file"))
                                .clicked()
                            {
                                self.prev_zip(ctx, false);
                            }
                            if ui
                                .button("⏭📦")
                                .on_hover_text(tr("toolbar.next_file"))
                                .clicked()
                            {
                                self.next_zip(ctx);
                            }
                            ui.separator();

                            // --- Page Navigation ---
                            if ui
                                .button("⏮")
                                .on_hover_text(tr("toolbar.first_page"))
                                .clicked()
                            {
                                self.go_to_first_page(ctx);
                            }
                            if ui
                                .button("◀")
                                .on_hover_text(tr("toolbar.prev_page"))
                                .clicked()
                            {
                                self.prev_page(ctx, false);
                            }

                            // Page Indicator in middle
                            ui.label(format!(
                                "{} / {}",
                                self.current_index + 1,
                                self.image_files.len()
                            ));

                            if ui
                                .button("▶")
                                .on_hover_text(tr("toolbar.next_page"))
                                .clicked()
                            {
                                self.next_page(ctx, false);
                            }
                            if ui
                                .button("⏭")
                                .on_hover_text(tr("toolbar.last_page"))
                                .clicked()
                            {
                                self.go_to_last_page(ctx);
                            }
                            ui.separator();

                            if ui
                                .button("1◀")
                                .on_hover_text(tr("toolbar.prev_page"))
                                .clicked()
                            {
                                self.prev_page(ctx, true);
                            }

                            if ui
                                .button("▶1")
                                .on_hover_text(tr("toolbar.next_page"))
                                .clicked()
                            {
                                self.next_page(ctx, true);
                            }
                            ui.separator();

                            // --- View Toggles ---
                            let shift_label = if self.is_shifted {
                                tr("toolbar.odd")
                            } else {
                                tr("toolbar.even")
                            };
                            let shift_tooltip = if self.is_shifted {
                                tr("state.odd_page")
                            } else {
                                tr("state.even_page")
                            };
                            if ui
                                .button(shift_label)
                                .on_hover_text(shift_tooltip)
                                .clicked()
                            {
                                self.change_shifted_mode(ctx);
                            }

                            if ui
                                .button(if self.is_fullscreen { "🗗" } else { "⛶" })
                                .on_hover_text(tr("toolbar.fullscreen"))
                                .clicked()
                            {
                                self.toggle_fullscreen(ctx);
                            }
                            ui.separator();
                            if ui
                                .button("📂")
                                .on_hover_text(tr("toolbar.open_file"))
                                .clicked()
                            {
                                self.open_file_dialog();
                            }
                            self.render_history_menu(ui, ctx);

                            ui.separator();
                            let auto_scroll_label = if self.auto_scroll_enabled {
                                "⏸"
                            } else {
                                "▶"
                            };
                            let auto_scroll_tooltip = if self.auto_scroll_enabled {
                                tr("toolbar.auto_scroll_stop")
                            } else {
                                tr("toolbar.auto_scroll_start")
                            };
                            if ui
                                .add_enabled(
                                    !self.image_files.is_empty(),
                                    egui::Button::new(auto_scroll_label),
                                )
                                .on_hover_text(auto_scroll_tooltip)
                                .clicked()
                            {
                                self.toggle_auto_scroll(ctx);
                            }

                            let mut auto_scroll_delay_ms = self.auto_scroll_delay_ms();
                            let delay_slider = egui::Slider::new(
                                &mut auto_scroll_delay_ms,
                                AUTO_SCROLL_MIN_DELAY_MS..=AUTO_SCROLL_MAX_DELAY_MS,
                            )
                            .text(tr("toolbar.auto_scroll_delay"));
                            let delay_response = ui.add_sized(
                                [180.0 * toolbar_scale, ui.spacing().interact_size.y],
                                delay_slider,
                            );
                            if delay_response.changed() {
                                self.config.image_delay = auto_scroll_delay_ms;
                                self.save_settings();
                                if self.auto_scroll_enabled {
                                    self.schedule_auto_scroll(ctx);
                                }
                            }
                            delay_response.on_hover_text(tr("toolbar.auto_scroll_delay.tooltip"));

                            if ui
                                .add_enabled(!self.image_files.is_empty(), egui::Button::new("↻"))
                                .on_hover_text(tr("toolbar.reload_current_image.tooltip"))
                                .clicked()
                            {
                                self.reload_current_image(ctx);
                            }

                            ui.separator();
                            // --- THE SLIDER ---
                            // We use a 1-based slider for better user experience
                            let mut page_val = self.current_index + 1;
                            let max_pages = self.image_files.len().max(1);

                            // ui.available_width() ensures the slider stretches to fill the gap
                            let slider_width = ui.available_width() / 3.0; // Reserve space for right-side buttons

                            ui.style_mut().spacing.slider_width = slider_width;

                            let slider = ui.add(
                                egui::Slider::new(&mut page_val, 1..=max_pages)
                                    .show_value(true)
                                    .text(format!("/ {}", max_pages)),
                            );
                            self.is_scrubbing = slider.dragged();
                            if slider.changed() {
                                self.current_index = page_val - 1;
                                self.reset_buffer();
                                self.reset_pan();
                                self.load_current_pair(ctx);
                            }

                            // --- Hide Button ---
                            // --- Hide Button ---
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("❌").on_hover_text(tr("toolbar.hide")).clicked()
                                    {
                                        self.config.show_top_bar = false;
                                    }
                                    if ui.button("⚙").on_hover_text(tr("settings.title")).clicked()
                                    {
                                        self.config.show_settings = !self.config.show_settings;
                                    }
                                },
                            );
                        });
                    });
                });
        }
    }
}
