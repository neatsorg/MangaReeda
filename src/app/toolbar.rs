// このファイルは RustMangaReader（Lycoris52、Apache License 2.0）のコードを元に改変したものです。

use super::{
    FULLSCREEN_TITLE_BAR_HEIGHT, FULLSCREEN_TOP_CHROME_REVEAL_HEIGHT, MENU_BAR_HEIGHT, MangaReader,
};
use crate::config::MangaAction;
use crate::engine::thumbnail::ThumbnailMode;
use crate::localize::tr;
use eframe::egui;

/// 履歴メニューに出す冊の数
const HISTORY_MENU_ITEMS: usize = 20;

impl MangaReader {
    pub(super) fn set_fullscreen(&mut self, fullscreen: bool, ctx: &egui::Context) {
        self.is_fullscreen = fullscreen;
        if !fullscreen {
            self.fullscreen_top_chrome_visible = false;
            self.fullscreen_bottom_toolbar_visible = false;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
    }

    pub(super) fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.set_fullscreen(!self.is_fullscreen, ctx);
    }

    /// 全画面で、マウスを画面の端に近づけたときだけタイトルバーとツールバーを出す。
    /// タイトルバーは上端、ツールバーは置いている側の端で出す
    pub(super) fn update_fullscreen_top_chrome_visibility(&mut self, ctx: &egui::Context) {
        if !self.is_fullscreen {
            self.fullscreen_top_chrome_visible = false;
            self.fullscreen_bottom_toolbar_visible = false;
            return;
        }

        let toolbar_scale = self.config.top_toolbar_scale.clamp(1.0, 3.0);
        let toolbar_height = 42.0 * toolbar_scale;
        let at_bottom = self.config.toolbar_at_bottom;
        // 出しているあいだは、出ているものの上にマウスがあれば出したままにする
        let reveal = |visible: bool, height: f32| {
            if visible {
                height
            } else {
                FULLSCREEN_TOP_CHROME_REVEAL_HEIGHT
            }
        };
        let top_height = reveal(
            self.fullscreen_top_chrome_visible,
            FULLSCREEN_TITLE_BAR_HEIGHT
                + MENU_BAR_HEIGHT
                + if at_bottom { 0.0 } else { toolbar_height },
        );
        let bottom_height = reveal(self.fullscreen_bottom_toolbar_visible, toolbar_height);
        let screen = ctx.content_rect();
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let show_top = pointer.is_some_and(|pos| pos.y <= screen.top() + top_height);
        let show_bottom =
            at_bottom && pointer.is_some_and(|pos| pos.y >= screen.bottom() - bottom_height);

        if self.fullscreen_top_chrome_visible != show_top
            || self.fullscreen_bottom_toolbar_visible != show_bottom
        {
            self.fullscreen_top_chrome_visible = show_top;
            self.fullscreen_bottom_toolbar_visible = show_bottom;
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
                    let title = self.window_title.clone();
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
                    // 全画面を解除する（🗗 は同梱のフォントにも egui の予備のフォントにも無く、四角になる）
                    if ui
                        .button("▣")
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

    /// ツールバー。設定に従って画面の下（ミーヤと同じ）か上に置く
    pub(super) fn render_toolbar(&mut self, ctx: &egui::Context) {
        let revealed = if self.config.toolbar_at_bottom {
            self.fullscreen_bottom_toolbar_visible
        } else {
            self.fullscreen_top_chrome_visible
        };
        let show_toolbar = self.config.show_top_bar && (!self.is_fullscreen || revealed);

        if show_toolbar {
            let panel = if self.config.toolbar_at_bottom {
                egui::TopBottomPanel::bottom("toolbar")
            } else {
                egui::TopBottomPanel::top("toolbar")
            };
            panel
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

                        // 窓の中で読んでいる（読むプラグインの本）ときは、ページの操作をその本に送り、
                        // 当てはまらないボタンは押せなくする
                        let web = self.web_view.is_some();
                        ui.horizontal(|ui| {
                            // --- Folder Navigation ---
                            if ui
                                .add_enabled(!web, egui::Button::new("📁⏮"))
                                .on_hover_text(tr("toolbar.prev_folder"))
                                .clicked()
                            {
                                self.prev_folder(ctx);
                            }
                            if ui
                                .add_enabled(!web, egui::Button::new("⏭📁"))
                                .on_hover_text(tr("toolbar.next_folder"))
                                .clicked()
                            {
                                self.next_folder(ctx);
                            }
                            ui.separator();

                            // --- File Navigation ---
                            if ui
                                .add_enabled(!web, egui::Button::new("📦⏮"))
                                .on_hover_text(tr("toolbar.prev_file"))
                                .clicked()
                            {
                                self.prev_zip(ctx, false);
                            }
                            if ui
                                .add_enabled(!web, egui::Button::new("⏭📦"))
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
                                if web {
                                    self.execute_action(MangaAction::FirstPage, ctx);
                                } else {
                                    self.go_to_first_page(ctx);
                                }
                            }
                            if ui
                                .button("◀")
                                .on_hover_text(tr("toolbar.prev_page"))
                                .clicked()
                            {
                                if web {
                                    self.execute_action(MangaAction::PrevPage, ctx);
                                } else {
                                    self.prev_page(ctx, false);
                                }
                            }

                            // Page Indicator in middle
                            if web {
                                let page = self.web_view.as_ref().and_then(|view| view.page);
                                ui.label(match page {
                                    Some([current, total]) => format!("{current} / {total}"),
                                    None => "- / -".to_owned(),
                                });
                            } else {
                                ui.label(format!(
                                    "{} / {}",
                                    self.current_index + 1,
                                    self.image_files.len()
                                ));
                            }

                            if ui
                                .button("▶")
                                .on_hover_text(tr("toolbar.next_page"))
                                .clicked()
                            {
                                if web {
                                    self.execute_action(MangaAction::NextPage, ctx);
                                } else {
                                    self.next_page(ctx, false);
                                }
                            }
                            if ui
                                .button("⏭")
                                .on_hover_text(tr("toolbar.last_page"))
                                .clicked()
                            {
                                if web {
                                    self.execute_action(MangaAction::LastPage, ctx);
                                } else {
                                    self.go_to_last_page(ctx);
                                }
                            }
                            ui.separator();

                            if ui
                                .button("1◀")
                                .on_hover_text(tr("toolbar.prev_page"))
                                .clicked()
                            {
                                if web {
                                    self.execute_action(MangaAction::OnePrevPage, ctx);
                                } else {
                                    self.prev_page(ctx, true);
                                }
                            }

                            if ui
                                .button("▶1")
                                .on_hover_text(tr("toolbar.next_page"))
                                .clicked()
                            {
                                if web {
                                    self.execute_action(MangaAction::OneNextPage, ctx);
                                } else {
                                    self.next_page(ctx, true);
                                }
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
                                .add_enabled(!web, egui::Button::new(shift_label))
                                .on_hover_text(shift_tooltip)
                                .clicked()
                            {
                                self.change_shifted_mode(ctx);
                            }

                            if ui
                                .button(if self.is_fullscreen { "▣" } else { "⛶" })
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
                            if ui
                                .add_enabled(!web && !self.image_files.is_empty(), egui::Button::new("🖼"))
                                .on_hover_text(tr("toolbar.thumbnails"))
                                .clicked()
                            {
                                self.open_thumbnails(ThumbnailMode::FitAll);
                            }
                            if ui
                                .add_enabled(web || self.zip_path.is_some(), egui::Button::new("📚"))
                                .on_hover_text(tr("toolbar.cover_thumbnails"))
                                .clicked()
                            {
                                if web {
                                    self.execute_action(MangaAction::CoverThumbnails, ctx);
                                } else {
                                    self.toggle_cover_thumbnails();
                                }
                            }

                            ui.separator();
                            if ui
                                .add_enabled(!web && !self.image_files.is_empty(), egui::Button::new("↻"))
                                .on_hover_text(tr("toolbar.reload_current_image.tooltip"))
                                .clicked()
                            {
                                self.reload_current_image(ctx);
                            }

                            ui.separator();
                            // シークバー。右端の ⚙ とページの値を置いた残りの幅いっぱいに広げる
                            // （ページ数が多い冊でも、細かく動かせるように）
                            let max_pages = self.image_files.len().max(1);
                            let mut page_val = (self.current_index + 1).min(max_pages);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("⚙").on_hover_text(tr("settings.title")).clicked()
                                    {
                                        self.config.show_settings = !self.config.show_settings;
                                    }
                                    if web {
                                        // 窓の中で読んでいる本。プラグインが知らせた位置を出し、
                                        // 離したとき（クリックや数字の入力はその場で）に飛ぶ。
                                        // ドラッグのあいだは数字だけ動かす（飛ぶたびにページを読み込ませないように）。
                                        // 位置が届くまでは押せない形だけ出す
                                        let (page, seek) = self
                                            .web_view
                                            .as_ref()
                                            .map_or((None, None), |view| (view.page, view.seek));
                                        let [current, total] = page.unwrap_or([1, 1]);
                                        let mut page_val = seek.unwrap_or(current).clamp(1, total);
                                        ui.label(if page.is_some() {
                                            format!("/ {total}")
                                        } else {
                                            "/ -".to_owned()
                                        });
                                        let typed = ui.add_enabled(
                                            page.is_some(),
                                            egui::DragValue::new(&mut page_val).range(1..=total),
                                        );
                                        ui.spacing_mut().slider_width = ui.available_width();
                                        let slider = ui.add_enabled(
                                            page.is_some(),
                                            egui::Slider::new(&mut page_val, 1..=total)
                                                .show_value(false),
                                        );
                                        self.is_scrubbing = false;
                                        let dragging = typed.dragged() || slider.dragged();
                                        let released = typed.drag_stopped()
                                            || slider.drag_stopped()
                                            || ((typed.changed() || slider.changed()) && !dragging);
                                        if released {
                                            self.web_view_go_to_page(page_val);
                                        } else if dragging
                                            && let Some(view) = self.web_view.as_mut()
                                        {
                                            view.seek = Some(page_val);
                                        }
                                        return;
                                    }
                                    ui.label(format!("/ {max_pages}"));
                                    let typed = ui
                                        .add(
                                            egui::DragValue::new(&mut page_val)
                                                .range(1..=max_pages),
                                        )
                                        .changed();
                                    ui.spacing_mut().slider_width = ui.available_width();
                                    let slider = ui.add(
                                        egui::Slider::new(&mut page_val, 1..=max_pages)
                                            .show_value(false),
                                    );
                                    self.is_scrubbing = slider.dragged();
                                    if typed || slider.changed() {
                                        // ほかのページ移動と同じく、分割表示の半分などの状態も戻す
                                        self.go_to_page(page_val - 1, ctx);
                                    }
                                },
                            );
                        });
                    });
                });
        }
    }
}
