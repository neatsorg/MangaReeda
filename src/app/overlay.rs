use super::MangaReader;
use crate::localize::tr;
use eframe::egui;

impl MangaReader {
    pub(super) fn render_error_overlay(&mut self, ctx: &egui::Context, ui: &egui::Ui) {
        // Error Overlay Logic (Fading)
        if let Some((msg, start_time)) = &self.error_msg {
            let elapsed = start_time.elapsed().as_secs_f32();
            if elapsed < 2.0 {
                let opacity = (1.0 - (elapsed / 2.0)).clamp(0.0, 1.0);
                let padding = if self.config.show_settings {
                    -(self.config.settings_width / 2.0)
                } else {
                    0.0
                };
                egui::Window::new("")
                    .anchor(egui::Align2::CENTER_TOP, [padding, 20.0]) // Positioned at top center
                    .frame(
                        egui::Frame::window(&ui.style())
                            .fill(egui::Color32::from_black_alpha((180.0 * opacity) as u8))
                            .stroke(egui::Stroke::new(
                                1.0,
                                egui::Color32::from_white_alpha((50.0 * opacity) as u8),
                            )),
                    )
                    .title_bar(false)
                    .show(ctx, |ui| {
                        ui.label(
                            egui::RichText::new(msg)
                                .color(egui::Color32::from_white_alpha((255.0 * opacity) as u8))
                                .size(24.0)
                                .strong(),
                        );
                    });
                ctx.request_repaint();
            } else {
                self.error_msg = None;
            }
        }
    }

    pub(super) fn render_zip_name_overlay(&mut self, ctx: &egui::Context, ui: &egui::Ui) {
        // --- ZIP FILENAME OVERLAY (Center-Top) ---
        if let Some((name, start_time)) = &self.zip_name_display {
            let elapsed = start_time.elapsed().as_secs_f32();

            if elapsed < 2.0 {
                let opacity = (1.0 - (elapsed / 2.0)).clamp(0.0, 1.0);
                let padding = if self.config.show_settings {
                    -(self.config.settings_width / 2.0)
                } else {
                    0.0
                };
                egui::Window::new("zip_name_overlay")
                    .anchor(egui::Align2::CENTER_TOP, [padding, 80.0]) // Positioned at top center
                    .frame(
                        egui::Frame::window(&ui.style())
                            .fill(egui::Color32::from_black_alpha((180.0 * opacity) as u8))
                            .stroke(egui::Stroke::new(
                                1.0,
                                egui::Color32::from_white_alpha((50.0 * opacity) as u8),
                            )),
                    )
                    .title_bar(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label(
                            egui::RichText::new(name)
                                .color(egui::Color32::from_white_alpha((255.0 * opacity) as u8))
                                .size(24.0)
                                .strong(),
                        );
                    });
                ctx.request_repaint(); // Keep the animation smooth
            } else {
                self.zip_name_display = None;
            }
        }
    }

    pub(super) fn render_page_indicator_overlay(&mut self, ctx: &egui::Context) {
        // --- THE PAGE INDICATOR OVERLAY (Large & Single Line) ---
        if let Some(start_time) = self.page_indicator_time {
            let elapsed = start_time.elapsed().as_secs_f32();
            if elapsed < 2.0 {
                let opacity = (1.0 - (elapsed / 2.0)).clamp(0.0, 1.0);
                let padding = if self.config.show_settings {
                    -30.0 - self.config.settings_width
                } else {
                    -15.0
                };
                egui::Window::new("page_info")
                    .anchor(egui::Align2::RIGHT_TOP, [padding, 10.0])
                    .frame(
                        egui::Frame::NONE
                            .fill(egui::Color32::from_rgba_unmultiplied(
                                60,
                                60,
                                60,
                                (opacity * 255.0) as u8,
                            ))
                            .inner_margin(5.0)
                            .corner_radius(5.0),
                    ) // No background box
                    .title_bar(false)
                    .resizable(false)
                    .collapsible(false)
                    .fixed_size([300.0, 60.0]) // Force a wide area to prevent wrapping
                    .show(ctx, |ui| {
                        // Ensure text stays on one line
                        ui.horizontal(|ui| {
                            let page_text =
                                format!("{} / {}", self.current_index + 1, self.image_files.len());
                            ui.label(
                                egui::RichText::new(page_text)
                                    .color(egui::Color32::from_white_alpha((200.0 * opacity) as u8))
                                    .size(22.0) // Much larger font
                                    .strong(),
                            );
                        });
                    });
                ctx.request_repaint();
            } else {
                self.page_indicator_time = None;
            }
        }
    }
}

impl MangaReader {
    /// サブフォルダを読むかを聞くダイアログ。外側のクリックや Esc はキャンセル
    pub(super) fn render_subfolder_question(&mut self, ctx: &egui::Context) {
        let Some(question) = self.subfolder_question.as_mut() else {
            return;
        };
        let name = question
            .folder
            .file_name()
            .map_or_else(
                || question.folder.to_string_lossy(),
                |name| name.to_string_lossy(),
            )
            .to_string();
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::new("subfolder_question")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(tr("subfolder.title"));
            ui.add_space(8.0);
            ui.label(tr("subfolder.message").replace("{name}", &name));
            ui.add_space(8.0);
            ui.checkbox(&mut question.remember, tr("subfolder.remember"));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(tr("subfolder.include")).clicked() {
                    answer = Some(Some(true));
                }
                if ui.button(tr("subfolder.exclude")).clicked() {
                    answer = Some(Some(false));
                }
                if ui.button(tr("subfolder.cancel")).clicked() {
                    answer = Some(None);
                }
            });
        });
        if answer.is_none() && response.should_close() {
            answer = Some(None);
        }
        if let Some(include) = answer {
            self.answer_subfolder_question(include, ctx);
        }
    }
}
