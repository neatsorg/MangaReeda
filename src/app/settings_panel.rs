// このファイルは RustMangaReader（Lycoris52、Apache License 2.0）のコードを元に改変したものです。

use super::{AUTO_SCROLL_MAX_DELAY_MS, MangaReader, ZOOM_MAX, ZOOM_MIN};
use crate::config::{
    AppSettings, GamepadButton, GamepadConfig, ImageSizingMode, KeyConfig, LastPageAction,
    MangaAction, MouseConfig, PageViewOptions, ResizeMethod, Shortcut, SubfolderMode, UiLanguage,
};
use crate::localize::tr;
use crate::utils::{
    action_label, gamepad_button_label, language_label, render_mouse_action_dropdown, separator_pct,
};
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ControlProfile {
    Default,
    TopDown,
}

pub(super) fn control_profile_header_suffix(profile: ControlProfile) -> &'static str {
    match profile {
        ControlProfile::Default => tr("settings.control.mode_default"),
        ControlProfile::TopDown => tr("settings.control.mode_top_down"),
    }
}

pub(super) fn control_mapping_header(base_label: &'static str, profile: ControlProfile) -> String {
    format!(
        "{} ({})",
        tr(base_label),
        control_profile_header_suffix(profile)
    )
}

#[derive(Clone, PartialEq)]
pub(super) enum BindingTarget {
    Keyboard {
        profile: ControlProfile,
        action: String,
    },
}

pub(super) fn render_binding_button(
    ui: &mut egui::Ui,
    id: &str,
    shortcut: &mut Option<Shortcut>,
    binding: &mut Option<BindingTarget>,
    profile: ControlProfile,
    listening_text: &str,
) {
    let is_binding = matches!(
        binding,
        Some(BindingTarget::Keyboard { profile: binding_profile, action })
            if *binding_profile == profile && action == id
    );
    let text = if is_binding {
        listening_text.to_string()
    } else {
        shortcut
            .map(|shortcut| shortcut.format())
            .unwrap_or_else(|| tr("common.unassigned").to_owned())
    };

    if ui.button(text).clicked() {
        *binding = Some(BindingTarget::Keyboard {
            profile,
            action: id.to_string(),
        });
    }
}

pub(super) fn gamepad_action_mut(
    config: &mut GamepadConfig,
    button: GamepadButton,
) -> &mut MangaAction {
    match button {
        GamepadButton::South => &mut config.south,
        GamepadButton::East => &mut config.east,
        GamepadButton::North => &mut config.north,
        GamepadButton::West => &mut config.west,
        GamepadButton::LeftTrigger => &mut config.left_trigger,
        GamepadButton::LeftTrigger2 => &mut config.left_trigger2,
        GamepadButton::RightTrigger => &mut config.right_trigger,
        GamepadButton::RightTrigger2 => &mut config.right_trigger2,
        GamepadButton::Select => &mut config.select,
        GamepadButton::Start => &mut config.start,
        GamepadButton::LeftThumb => &mut config.left_thumb,
        GamepadButton::RightThumb => &mut config.right_thumb,
        GamepadButton::DPadUp => &mut config.dpad_up,
        GamepadButton::DPadDown => &mut config.dpad_down,
        GamepadButton::DPadLeft => &mut config.dpad_left,
        GamepadButton::DPadRight => &mut config.dpad_right,
    }
}

pub(super) fn render_control_mapping_settings(
    ui: &mut egui::Ui,
    profile: ControlProfile,
    keys: &mut KeyConfig,
    mouse: &mut MouseConfig,
    gamepad: &mut GamepadConfig,
    double_click_threshold_ms: &mut u64,
    binding_action: &mut Option<BindingTarget>,
    mangameeya_requested: &mut bool,
) -> bool {
    let mut changed = false;
    let listening_text = tr("common.listening").to_owned();

    // 操作の割り当てをまとめて差し替えるボタン
    ui.horizontal_wrapped(|ui| {
        // 通常表示と縦スクロール表示の両方を変えるので、ここでは押されたことだけを伝える
        if ui
            .button(tr("settings.control.preset_mangameeya"))
            .on_hover_text(tr("settings.control.preset_mangameeya.tooltip"))
            .clicked()
        {
            *binding_action = None;
            *mangameeya_requested = true;
        }
        if ui
            .button(tr("settings.control.preset_reset"))
            .on_hover_text(tr("settings.control.preset_reset.tooltip"))
            .clicked()
        {
            let defaults = AppSettings::default();
            match profile {
                ControlProfile::Default => {
                    *keys = defaults.keys;
                    *mouse = defaults.mouse;
                    *gamepad = defaults.gamepad;
                    *double_click_threshold_ms = defaults.double_click_threshold_ms;
                }
                ControlProfile::TopDown => {
                    *keys = defaults.top_down_keys;
                    *mouse = defaults.top_down_mouse;
                    *gamepad = defaults.top_down_gamepad;
                    *double_click_threshold_ms = defaults.top_down_double_click_threshold_ms;
                }
            }
            *binding_action = None;
            changed = true;
        }
    });

    egui::CollapsingHeader::new(
        egui::RichText::new(control_mapping_header("settings.key_config", profile))
            .size(20.0)
            .strong(),
    )
    .default_open(true)
    .show(ui, |ui| {
        separator_pct(ui);
        egui::Grid::new(format!("key_grid_{profile:?}"))
            .num_columns(2)
            .spacing([20.0, 10.0])
            .show(ui, |ui| {
                for (label, id, shortcut) in [
                    (
                        tr("settings.key.slide_image_down"),
                        "Slide Image Down",
                        &mut keys.slide_image_down,
                    ),
                    (
                        tr("settings.key.slide_image_up"),
                        "Slide Image Up",
                        &mut keys.slide_image_up,
                    ),
                    (
                        tr("settings.key.toggle_auto_scroll"),
                        "Toggle Auto Scroll",
                        &mut keys.toggle_auto_scroll,
                    ),
                    (
                        tr("settings.key.reload_current_image"),
                        "Reload Current Image",
                        &mut keys.reload_current_image,
                    ),
                    (
                        tr("settings.key.next_page"),
                        "Next Page",
                        &mut keys.next_page,
                    ),
                    (
                        tr("settings.key.prev_page"),
                        "Previous Page",
                        &mut keys.prev_page,
                    ),
                    (
                        tr("settings.key.one_next_page"),
                        "1 Next Page",
                        &mut keys.one_next_page,
                    ),
                    (
                        tr("settings.key.one_prev_page"),
                        "1 Prev Page",
                        &mut keys.one_prev_page,
                    ),
                    (
                        tr("settings.key.first_page"),
                        "First Page",
                        &mut keys.first_page,
                    ),
                    (
                        tr("settings.key.last_page"),
                        "Last Page",
                        &mut keys.last_page,
                    ),
                    (
                        tr("settings.key.next_file"),
                        "Next File",
                        &mut keys.next_file,
                    ),
                    (
                        tr("settings.key.prev_file"),
                        "Previous File",
                        &mut keys.prev_file,
                    ),
                    (
                        tr("settings.key.next_folder"),
                        "Next Folder",
                        &mut keys.next_folder,
                    ),
                    (
                        tr("settings.key.prev_folder"),
                        "Previous Folder",
                        &mut keys.prev_folder,
                    ),
                    (
                        tr("settings.key.fullscreen"),
                        "Toggle Fullscreen",
                        &mut keys.fullscreen,
                    ),
                    (
                        tr("settings.key.view_mode"),
                        "View Mode",
                        &mut keys.view_mode,
                    ),
                    (
                        tr("settings.key.open_file"),
                        "Open File",
                        &mut keys.open_file,
                    ),
                    (tr("settings.key.quit_app"), "Quit App", &mut keys.quit_app),
                    (
                        tr("settings.key.cycle_last_page_action"),
                        "Cycle Last Page Action",
                        &mut keys.cycle_last_page_action,
                    ),
                    (
                        tr("settings.key.scroll_next_page"),
                        "Scroll Next Page",
                        &mut keys.scroll_next_page,
                    ),
                    (
                        tr("settings.key.scroll_prev_page"),
                        "Scroll Prev Page",
                        &mut keys.scroll_prev_page,
                    ),
                    (
                        tr("settings.key.scroll_up"),
                        "Scroll Up",
                        &mut keys.scroll_up,
                    ),
                    (
                        tr("settings.key.scroll_down"),
                        "Scroll Down",
                        &mut keys.scroll_down,
                    ),
                    (
                        tr("settings.key.scroll_left"),
                        "Scroll Left",
                        &mut keys.scroll_left,
                    ),
                    (
                        tr("settings.key.scroll_right"),
                        "Scroll Right",
                        &mut keys.scroll_right,
                    ),
                    (
                        tr("settings.key.toggle_loupe"),
                        "Toggle Loupe",
                        &mut keys.toggle_loupe,
                    ),
                    (
                        tr("settings.key.toggle_auto_page_layout"),
                        "Toggle Auto Page Layout",
                        &mut keys.toggle_auto_page_layout,
                    ),
                    (
                        tr("settings.key.jump_forward"),
                        "Jump Forward",
                        &mut keys.jump_forward,
                    ),
                    (
                        tr("settings.key.jump_backward"),
                        "Jump Backward",
                        &mut keys.jump_backward,
                    ),
                    (
                        tr("settings.key.toggle_split_pages"),
                        "Toggle Split Pages",
                        &mut keys.toggle_split_pages,
                    ),
                    (
                        tr("settings.key.thumbnails_fit_all"),
                        "Thumbnails Fit All",
                        &mut keys.thumbnails_fit_all,
                    ),
                    (
                        tr("settings.key.thumbnails_large"),
                        "Thumbnails Large",
                        &mut keys.thumbnails_large,
                    ),
                    (
                        tr("settings.key.cover_thumbnails"),
                        "Cover Thumbnails",
                        &mut keys.cover_thumbnails,
                    ),
                    (
                        tr("settings.key.upper_folder"),
                        "Upper Folder",
                        &mut keys.upper_folder,
                    ),
                    (
                        tr("settings.key.show_history"),
                        "Show History",
                        &mut keys.show_history,
                    ),
                    (
                        tr("settings.key.show_favorites"),
                        "Show Favorites",
                        &mut keys.show_favorites,
                    ),
                    (
                        tr("settings.key.toggle_favorite"),
                        "Toggle Favorite",
                        &mut keys.toggle_favorite,
                    ),
                ] {
                    ui.label(label);
                    render_binding_button(
                        ui,
                        id,
                        shortcut,
                        binding_action,
                        profile,
                        &listening_text,
                    );
                    ui.end_row();
                }
            });
        separator_pct(ui);
    });

    ui.add_space(20.0);
    egui::CollapsingHeader::new(
        egui::RichText::new(control_mapping_header("settings.mouse_mapping", profile))
            .size(20.0)
            .strong(),
    )
    .default_open(true)
    .show(ui, |ui| {
        separator_pct(ui);
        ui.label(tr("settings.mouse_mapping.description"));
        ui.add_space(10.0);
        changed |= ui
            .add(
                egui::Slider::new(double_click_threshold_ms, 100..=1000)
                    .text(tr("settings.double_click_threshold")),
            )
            .changed();
        ui.add_space(10.0);
        let action_options: Vec<(MangaAction, String)> = MangaAction::ALL
            .into_iter()
            .map(|action| (action, action_label(action).to_owned()))
            .collect();
        let unassigned_label = tr("common.unassigned").to_owned();
        egui::Grid::new(format!("mouse_grid_{profile:?}"))
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                for (label, id, action) in [
                    (
                        tr("settings.mouse.scroll_up"),
                        "mouse_scroll_up",
                        &mut mouse.scroll_up,
                    ),
                    (
                        tr("settings.mouse.scroll_down"),
                        "mouse_scroll_down",
                        &mut mouse.scroll_down,
                    ),
                    (
                        tr("settings.mouse.button1_click"),
                        "mouse_button1_click",
                        &mut mouse.button1_click,
                    ),
                    (
                        tr("settings.mouse.button2_click"),
                        "mouse_button2_click",
                        &mut mouse.button2_click,
                    ),
                    (
                        tr("settings.mouse.button3_click"),
                        "mouse_button3_click",
                        &mut mouse.button3_click,
                    ),
                    (
                        tr("settings.mouse.button4_click"),
                        "mouse_button4_click",
                        &mut mouse.button4_click,
                    ),
                    (
                        tr("settings.mouse.button5_click"),
                        "mouse_button5_click",
                        &mut mouse.button5_click,
                    ),
                    (
                        tr("settings.mouse.button1_double_click"),
                        "mouse_button1_double_click",
                        &mut mouse.button1_double_click,
                    ),
                    (
                        tr("settings.mouse.button2_double_click"),
                        "mouse_button2_double_click",
                        &mut mouse.button2_double_click,
                    ),
                    (
                        tr("settings.mouse.button3_double_click"),
                        "mouse_button3_double_click",
                        &mut mouse.button3_double_click,
                    ),
                    (
                        tr("settings.mouse.button4_double_click"),
                        "mouse_button4_double_click",
                        &mut mouse.button4_double_click,
                    ),
                    (
                        tr("settings.mouse.button5_double_click"),
                        "mouse_button5_double_click",
                        &mut mouse.button5_double_click,
                    ),
                    (
                        tr("settings.mouse.button1_long_click"),
                        "mouse_button1_long_click",
                        &mut mouse.button1_long_click,
                    ),
                    (
                        tr("settings.mouse.button2_long_click"),
                        "mouse_button2_long_click",
                        &mut mouse.button2_long_click,
                    ),
                    (
                        tr("settings.mouse.button3_long_click"),
                        "mouse_button3_long_click",
                        &mut mouse.button3_long_click,
                    ),
                    (
                        tr("settings.mouse.button4_long_click"),
                        "mouse_button4_long_click",
                        &mut mouse.button4_long_click,
                    ),
                    (
                        tr("settings.mouse.button5_long_click"),
                        "mouse_button5_long_click",
                        &mut mouse.button5_long_click,
                    ),
                    (
                        tr("settings.mouse.button2_held_button1_click"),
                        "mouse_button2_held_button1_click",
                        &mut mouse.button2_held_button1_click,
                    ),
                    (
                        tr("settings.mouse.button1_held_button2_click"),
                        "mouse_button1_held_button2_click",
                        &mut mouse.button1_held_button2_click,
                    ),
                    (
                        tr("settings.mouse.button2_held_scroll_up"),
                        "mouse_button2_held_scroll_up",
                        &mut mouse.button2_held_scroll_up,
                    ),
                    (
                        tr("settings.mouse.button2_held_scroll_down"),
                        "mouse_button2_held_scroll_down",
                        &mut mouse.button2_held_scroll_down,
                    ),
                ] {
                    ui.label(label);
                    changed |= render_mouse_action_dropdown(
                        ui,
                        &format!("{id}_{profile:?}"),
                        action,
                        &action_options,
                        &unassigned_label,
                    );
                    ui.end_row();
                }
            });
        separator_pct(ui);
    });

    ui.add_space(20.0);
    egui::CollapsingHeader::new(
        egui::RichText::new(control_mapping_header("settings.gamepad_mapping", profile))
            .size(20.0)
            .strong(),
    )
    .default_open(true)
    .show(ui, |ui| {
        separator_pct(ui);
        ui.label(tr("settings.gamepad_mapping.description"));
        ui.add_space(10.0);
        let action_options: Vec<(MangaAction, String)> = MangaAction::ALL
            .into_iter()
            .map(|action| (action, action_label(action).to_owned()))
            .collect();
        let unassigned_label = tr("common.unassigned").to_owned();
        egui::Grid::new(format!("gamepad_grid_{profile:?}"))
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                for button in GamepadButton::ALL {
                    ui.label(gamepad_button_label(button));
                    changed |= render_mouse_action_dropdown(
                        ui,
                        &format!("gamepad_{profile:?}_{button:?}"),
                        gamepad_action_mut(gamepad, button),
                        &action_options,
                        &unassigned_label,
                    );
                    ui.end_row();
                }
            });
        separator_pct(ui);
    });

    changed
}

impl MangaReader {
    pub(super) fn render_settings_panel(&mut self, ctx: &egui::Context) {
        if self.config.show_settings {
            egui::SidePanel::right("settings_panel")
                .resizable(true) // Enable mouse dragging
                .default_width(self.config.settings_width)
                .width_range(150.0..=500.0) // Constraints
                .frame({
                    let v = ctx.style().visuals.clone();

                    // Use theme colors instead of fixed gray.
                    // Slight tint so it stands out from the main background in both modes.
                    let fill = if v.dark_mode {
                        v.window_fill.gamma_multiply(1.15)
                    } else {
                        v.window_fill.gamma_multiply(0.97)
                    };

                    egui::Frame::NONE
                        .fill(fill)
                        .stroke(v.window_stroke)
                        .inner_margin(egui::Margin::same(10))
                })
                .show(ctx, |ui| {
                    // Update the stored width based on user dragging
                    self.config.settings_width = ui.available_width();

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.heading(egui::RichText::new(tr("settings.title")).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // 同梱のフォントに ✕ が無いので ×（四角になってしまう）
                            if ui.button("×").on_hover_text(tr("settings.close")).clicked() {
                                self.config.show_settings = false;
                            }
                        });
                    });
                    ui.add_space(10.0);
                    separator_pct(ui);

                    // The rest becomes scrollable
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2]) // don't shrink content; enable scrolling
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(tr("language.label"))
                                        .size(20.0)
                                        .strong(),
                                );
                                separator_pct(ui);
                                let mut language_changed = false;
                                let english_label = language_label(UiLanguage::English).to_owned();
                                let japanese_label =
                                    language_label(UiLanguage::Japanese).to_owned();
                                egui::ComboBox::from_id_salt("language_select")
                                    .selected_text(language_label(self.config.language))
                                    .show_ui(ui, |ui| {
                                        language_changed |= ui
                                            .selectable_value(
                                                &mut self.config.language,
                                                UiLanguage::English,
                                                &english_label,
                                            )
                                            .changed();
                                        language_changed |= ui
                                            .selectable_value(
                                                &mut self.config.language,
                                                UiLanguage::Japanese,
                                                &japanese_label,
                                            )
                                            .changed();
                                    });
                                if language_changed {
                                    self.set_ui_language(self.config.language);
                                }

                                ui.add_space(20.0);
                                ui.label(
                                    egui::RichText::new(tr("settings.image_scaling"))
                                        .size(20.0)
                                        .strong(),
                                );
                                separator_pct(ui);

                                let visuals = ui.visuals_mut();
                                //visuals.selection.bg_fill = egui::Color32::BLACK;
                                visuals.override_text_color = None;

                                {
                                    let mut changed = false;
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.resize_method,
                                            ResizeMethod::None,
                                            egui::RichText::new(tr("settings.scaling.none")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.resize_method,
                                            ResizeMethod::Nearest,
                                            egui::RichText::new(tr("settings.scaling.nearest")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.resize_method,
                                            ResizeMethod::Triangle,
                                            egui::RichText::new(tr("settings.scaling.triangle")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.resize_method,
                                            ResizeMethod::CatmullRom,
                                            egui::RichText::new(tr("settings.scaling.catmullrom")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.resize_method,
                                            ResizeMethod::Lanczos3,
                                            egui::RichText::new(tr("settings.scaling.lanczos3")),
                                        )
                                        .clicked();

                                    if changed {
                                        self.set_resize_method(self.config.resize_method, ctx);
                                    }
                                }

                                ui.add_space(20.0);
                                ui.label(
                                    egui::RichText::new(tr("settings.page_view"))
                                        .size(20.0)
                                        .strong(),
                                );
                                separator_pct(ui);

                                {
                                    let mut chosen = self.config.page_view_options;
                                    let mut changed = false;
                                    changed |= ui
                                        .radio_value(
                                            &mut chosen,
                                            PageViewOptions::Single,
                                            egui::RichText::new(tr("settings.page_view.single")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut chosen,
                                            PageViewOptions::DoubleRL,
                                            egui::RichText::new(tr("settings.page_view.double_rl")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut chosen,
                                            PageViewOptions::DoubleLR,
                                            egui::RichText::new(tr("settings.page_view.double_lr")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut chosen,
                                            PageViewOptions::TopDown,
                                            egui::RichText::new(tr("settings.page_view.top_down")),
                                        )
                                        .clicked();
                                    let mut cover_blank = self.config.cover_blank_page;
                                    if ui
                                        .checkbox(&mut cover_blank, tr("settings.cover_blank_page"))
                                        .on_hover_text(tr("settings.cover_blank_page.tooltip"))
                                        .changed()
                                    {
                                        self.set_cover_blank_page(cover_blank, ctx);
                                    }
                                    let mut split = self.config.split_landscape_pages;
                                    if ui
                                        .checkbox(&mut split, tr("settings.split_landscape_pages"))
                                        .on_hover_text(tr("settings.split_landscape_pages.tooltip"))
                                        .changed()
                                    {
                                        self.toggle_split_landscape_pages(ctx);
                                    }
                                    separator_pct(ui);
                                    ui.label(
                                        egui::RichText::new(tr("settings.page_view.center_offset"))
                                            .size(20.0)
                                            .strong(),
                                    );
                                    let slider_width = ui.available_width() * 0.98;
                                    let previous_slider_width = ui.spacing().slider_width;
                                    ui.spacing_mut().slider_width = slider_width - 120.0;
                                    changed |= ui
                                        .add(
                                            egui::Slider::new(
                                                &mut self.config.spread_center_offset,
                                                -150.0..=150.0,
                                            )
                                            .step_by(1.0),
                                        )
                                        .on_hover_text(tr(
                                            "settings.page_view.center_offset.tooltip",
                                        ))
                                        .changed();
                                    ui.spacing_mut().slider_width = previous_slider_width - 40.0;

                                    if changed {
                                        self.set_page_view(chosen, ctx);
                                    }
                                }

                                ui.add_space(20.0);
                                ui.label(
                                    egui::RichText::new(tr("settings.last_page"))
                                        .size(20.0)
                                        .strong(),
                                );
                                separator_pct(ui);

                                {
                                    let mut changed = false;
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.last_page_action,
                                            LastPageAction::GotoNextFile,
                                            egui::RichText::new(tr("settings.last_page.next_file")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.last_page_action,
                                            LastPageAction::ToFirstPage,
                                            egui::RichText::new(tr(
                                                "settings.last_page.first_page",
                                            )),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.last_page_action,
                                            LastPageAction::Nothing,
                                            egui::RichText::new(tr("settings.last_page.nothing")),
                                        )
                                        .clicked();

                                    if changed {
                                        self.set_last_page_action(self.config.last_page_action);
                                    }
                                }

                                ui.add_space(20.0);
                                ui.label(
                                    egui::RichText::new(tr("settings.subfolder"))
                                        .size(20.0)
                                        .strong(),
                                )
                                .on_hover_text(tr("settings.subfolder.tooltip"));
                                separator_pct(ui);

                                {
                                    let mut changed = false;
                                    for (mode, label) in [
                                        (SubfolderMode::Ask, "settings.subfolder.ask"),
                                        (SubfolderMode::Include, "settings.subfolder.include"),
                                        (SubfolderMode::Exclude, "settings.subfolder.exclude"),
                                    ] {
                                        changed |= ui
                                            .radio_value(
                                                &mut self.config.subfolder_mode,
                                                mode,
                                                tr(label),
                                            )
                                            .clicked();
                                    }
                                    if changed {
                                        self.save_settings();
                                    }
                                }

                                ui.add_space(20.0);
                                ui.label(
                                    egui::RichText::new(tr("settings.zoom")).size(20.0).strong(),
                                );
                                separator_pct(ui);

                                let previous_slider_width = ui.spacing().slider_width;
                                ui.spacing_mut().slider_width =
                                    self.config.settings_width * 0.9 - 160.0;
                                let mut zoom_factor = self.zoom_factor;
                                let zoom_slider = ui.add(
                                    egui::Slider::new(&mut zoom_factor, ZOOM_MIN..=ZOOM_MAX)
                                        .text(tr("settings.zoom.slider")),
                                );
                                ui.spacing_mut().slider_width = previous_slider_width;
                                if zoom_slider.changed() {
                                    self.apply_zoom_factor(zoom_factor, ctx, false);
                                }

                                if ui
                                    .button(egui::RichText::new(tr("settings.zoom.reset")))
                                    .clicked()
                                {
                                    self.reset_pan();
                                    self.apply_zoom_factor(1.0, ctx, false);
                                }
                                ui.add_space(10.0);
                                let mut loupe_enabled = self.loupe_enabled;
                                if ui
                                    .checkbox(&mut loupe_enabled, tr("settings.loupe"))
                                    .changed()
                                {
                                    self.set_loupe_enabled(loupe_enabled, ctx);
                                }
                                let mut loupe_changed = false;
                                loupe_changed |= ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.config.loupe_radius,
                                            50.0..=400.0,
                                        )
                                        .text(tr("settings.loupe.radius")),
                                    )
                                    .changed();
                                loupe_changed |= ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.config.loupe_magnification,
                                            1.5..=6.0,
                                        )
                                        .text(tr("settings.loupe.magnification")),
                                    )
                                    .changed();
                                if loupe_changed {
                                    self.save_settings();
                                }
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(tr("settings.image_sizing"))
                                        .size(18.0)
                                        .strong(),
                                );
                                let mut image_sizing_changed = false;
                                image_sizing_changed |= ui
                                    .radio_value(
                                        &mut self.config.image_sizing_mode,
                                        ImageSizingMode::FitHeight,
                                        egui::RichText::new(tr("settings.image_sizing.fit_height")),
                                    )
                                    .clicked();
                                image_sizing_changed |= ui
                                    .radio_value(
                                        &mut self.config.image_sizing_mode,
                                        ImageSizingMode::FitWidth,
                                        egui::RichText::new(tr("settings.image_sizing.fit_width")),
                                    )
                                    .clicked();
                                image_sizing_changed |= ui
                                    .radio_value(
                                        &mut self.config.image_sizing_mode,
                                        ImageSizingMode::FitBoth,
                                        egui::RichText::new(tr("settings.image_sizing.fit_both")),
                                    )
                                    .clicked();
                                image_sizing_changed |= ui
                                    .radio_value(
                                        &mut self.config.image_sizing_mode,
                                        ImageSizingMode::MatchHeight,
                                        egui::RichText::new(tr(
                                            "settings.image_sizing.match_height",
                                        )),
                                    )
                                    .on_hover_text(tr("settings.image_sizing.match_height.tooltip"))
                                    .clicked();
                                image_sizing_changed |= ui
                                    .radio_value(
                                        &mut self.config.image_sizing_mode,
                                        ImageSizingMode::ShrinkOnly,
                                        egui::RichText::new(tr(
                                            "settings.image_sizing.shrink_only",
                                        )),
                                    )
                                    .on_hover_text(tr("settings.image_sizing.shrink_only.tooltip"))
                                    .clicked();
                                image_sizing_changed |= ui
                                    .radio_value(
                                        &mut self.config.image_sizing_mode,
                                        ImageSizingMode::OriginalSize,
                                        egui::RichText::new(tr("settings.image_sizing.original")),
                                    )
                                    .clicked();
                                if image_sizing_changed {
                                    self.set_image_sizing_mode(self.config.image_sizing_mode, ctx);
                                }
                                separator_pct(ui);

                                ui.add_space(20.0);
                                ui.label(
                                    egui::RichText::new(tr("settings.others"))
                                        .size(20.0)
                                        .strong(),
                                );
                                separator_pct(ui);
                                let mut rl = self.config.auto_page_layout_rl;
                                let mut lr = self.config.auto_page_layout_lr;
                                let rl_changed = ui
                                    .checkbox(&mut rl, tr("settings.auto_page_layout_rl"))
                                    .on_hover_text(tr("settings.auto_page_layout.tooltip"))
                                    .changed();
                                let lr_changed = ui
                                    .checkbox(&mut lr, tr("settings.auto_page_layout_lr"))
                                    .on_hover_text(tr("settings.auto_page_layout.tooltip"))
                                    .changed();
                                if rl_changed || lr_changed {
                                    let direction = if rl_changed {
                                        rl.then_some(true)
                                    } else {
                                        lr.then_some(false)
                                    };
                                    self.set_auto_page_layout(direction, ctx);
                                }
                                ui.horizontal(|ui| {
                                    ui.label(tr("settings.image_panel_background"));
                                    let mut background_color = self.image_panel_background_color();
                                    if egui::color_picker::color_edit_button_srgba(
                                        ui,
                                        &mut background_color,
                                        egui::color_picker::Alpha::Opaque,
                                    )
                                    .changed()
                                    {
                                        self.config.image_panel_background = [
                                            background_color.r(),
                                            background_color.g(),
                                            background_color.b(),
                                            background_color.a(),
                                        ];
                                        self.save_settings();
                                    }
                                });
                                ui.checkbox(
                                    &mut self.config.show_top_bar,
                                    tr("settings.show_toolbar"),
                                );
                                if ui
                                    .checkbox(
                                        &mut self.config.toolbar_at_bottom,
                                        tr("settings.toolbar_at_bottom"),
                                    )
                                    .changed()
                                {
                                    self.save_settings();
                                }
                                ui.checkbox(
                                    &mut self.config.transparency_support,
                                    tr("settings.transparency"),
                                )
                                .on_hover_text(tr("settings.transparency.tooltip"));
                                ui.checkbox(
                                    &mut self.config.enable_auto_image_byte_fix,
                                    tr("settings.auto_image_fix"),
                                )
                                .on_hover_text(tr("settings.auto_image_fix.tooltip"));
                                ui.checkbox(
                                    &mut self.config.enable_single_file_caching,
                                    tr("settings.single_file_cache"),
                                )
                                .on_hover_text(tr("settings.single_file_cache.tooltip"));
                                if ui
                                    .checkbox(
                                        &mut self.config.enable_animated_images,
                                        tr("settings.animated_images"),
                                    )
                                    .on_hover_text(tr("settings.animated_images.tooltip"))
                                    .changed()
                                {
                                    self.clear_animated_image_cache();
                                    self.clear_animation_scan_state();
                                    self.clear_active_animations();
                                    if self.config.enable_animated_images {
                                        self.restart_animation_scan_for_current_window();
                                    }
                                    self.save_settings();
                                }
                                let previous_slider_width = ui.spacing().slider_width;
                                ui.spacing_mut().slider_width =
                                    self.config.settings_width * 0.9 - 190.0;
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.config.image_delay,
                                            0..=AUTO_SCROLL_MAX_DELAY_MS,
                                        )
                                        .text(tr("settings.image_delay")),
                                    )
                                    .on_hover_text(tr("settings.image_delay.tooltip"))
                                    .changed()
                                {
                                    self.save_settings();
                                    // 自動ページ送りの間隔でもあるので、送っている途中なら次の時刻を決め直す
                                    if self.auto_scroll_enabled {
                                        self.schedule_auto_scroll(ui.ctx());
                                    }
                                }
                                ui.spacing_mut().slider_width = previous_slider_width;
                                let previous_slider_width = ui.spacing().slider_width;
                                ui.spacing_mut().slider_width =
                                    self.config.settings_width * 0.9 - 190.0;
                                let mut slide_speed_slider =
                                    (self.config.top_down_image_slide_speed.max(0.005) / 0.05)
                                        .log10()
                                        * 10.0;
                                slide_speed_slider = slide_speed_slider.clamp(-10.0, 10.0);
                                if ui
                                    .add(
                                        egui::Slider::new(&mut slide_speed_slider, -10.0..=10.0)
                                            .text(tr("settings.top_down_slide_speed")),
                                    )
                                    .on_hover_text(tr("settings.top_down_slide_speed.tooltip"))
                                    .changed()
                                {
                                    self.config.top_down_image_slide_speed =
                                        0.05 * 10.0_f32.powf(slide_speed_slider / 10.0);
                                    self.save_settings();
                                }
                                ui.spacing_mut().slider_width = previous_slider_width;
                                let previous_slider_width = ui.spacing().slider_width;
                                ui.spacing_mut().slider_width =
                                    self.config.settings_width * 0.9 - 190.0;
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.config.top_down_image_drag_speed,
                                            0.1..=5.0,
                                        )
                                        .text(tr("settings.top_down_drag_speed")),
                                    )
                                    .on_hover_text(tr("settings.top_down_drag_speed.tooltip"))
                                    .changed()
                                {
                                    self.save_settings();
                                }
                                ui.spacing_mut().slider_width = previous_slider_width;
                                let previous_slider_width = ui.spacing().slider_width;
                                ui.spacing_mut().slider_width =
                                    self.config.settings_width * 0.9 - 190.0;
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.config.top_toolbar_scale,
                                            1.0..=3.0,
                                        )
                                        .text(tr("settings.top_toolbar_scale")),
                                    )
                                    .on_hover_text(tr("settings.top_toolbar_scale.tooltip"))
                                    .changed()
                                {
                                    self.config.top_toolbar_scale =
                                        self.config.top_toolbar_scale.clamp(1.0, 3.0);
                                    self.save_settings();
                                }
                                ui.spacing_mut().slider_width = previous_slider_width;
                                ui.add_space(20.0);
                                ui.horizontal(|ui| {
                                    ui.selectable_value(
                                        &mut self.selected_control_profile,
                                        ControlProfile::Default,
                                        tr("settings.control.default_view"),
                                    );
                                    ui.selectable_value(
                                        &mut self.selected_control_profile,
                                        ControlProfile::TopDown,
                                        tr("settings.control.top_down_view"),
                                    );
                                });
                                separator_pct(ui);

                                let mut mangameeya_requested = false;
                                let controls_changed = match self.selected_control_profile {
                                    ControlProfile::Default => render_control_mapping_settings(
                                        ui,
                                        ControlProfile::Default,
                                        &mut self.config.keys,
                                        &mut self.config.mouse,
                                        &mut self.config.gamepad,
                                        &mut self.config.double_click_threshold_ms,
                                        &mut self.binding_action,
                                        &mut mangameeya_requested,
                                    ),
                                    ControlProfile::TopDown => render_control_mapping_settings(
                                        ui,
                                        ControlProfile::TopDown,
                                        &mut self.config.top_down_keys,
                                        &mut self.config.top_down_mouse,
                                        &mut self.config.top_down_gamepad,
                                        &mut self.config.top_down_double_click_threshold_ms,
                                        &mut self.binding_action,
                                        &mut mangameeya_requested,
                                    ),
                                };
                                if mangameeya_requested {
                                    self.config.apply_mangameeya_controls();
                                }
                                if controls_changed || mangameeya_requested {
                                    self.save_settings();
                                }

                                self.render_plugin_settings(ui);
                                ui.add_space(50.0);
                            });
                        });
                });
        }
    }
}
