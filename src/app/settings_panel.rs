use super::{
    AUTO_SCROLL_MAX_DELAY_MS, MangaReader, SETTINGS_TOGGLE_FADE_DURATION, ZOOM_MAX, ZOOM_MIN,
};
use crate::config::{
    AppSettings, GamepadButton, GamepadConfig, ImageSizingMode, KeyConfig, LastPageAction,
    MangaAction, MouseConfig, PageViewOptions, ResizeMethod, Shortcut, UiLanguage,
};
use crate::localize::tr;
use crate::utils::{
    action_label, gamepad_button_label, language_label, render_mouse_action_dropdown, separator_pct,
};
use eframe::egui;
use std::time::Instant;

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
                    ui.vertical_centered(|ui| {
                        ui.heading(egui::RichText::new(tr("settings.title")).strong());
                    });
                    ui.add_space(10.0);
                    separator_pct(ui);

                    // The rest becomes scrollable
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2]) // don't shrink content; enable scrolling
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.add_space(10.0);

                                // [Open File] Button
                                if ui
                                    .add_sized(
                                        [ui.available_width() * 0.94, 30.0],
                                        egui::Button::new(tr("settings.open_file")),
                                    )
                                    .clicked()
                                {
                                    self.open_file_dialog();
                                }
                                ui.add_space(20.0);
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
                                    self.save_settings();
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
                                        self.reset_buffer();
                                        self.texture_cache.clear();
                                        self.load_current_pair(ctx);
                                        self.save_settings(); // Save when algorithm changes
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
                                    let previous_page_view = self.config.page_view_options;
                                    let mut changed = false;
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.page_view_options,
                                            PageViewOptions::Single,
                                            egui::RichText::new(tr("settings.page_view.single")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.page_view_options,
                                            PageViewOptions::DoubleRL,
                                            egui::RichText::new(tr("settings.page_view.double_rl")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.page_view_options,
                                            PageViewOptions::DoubleLR,
                                            egui::RichText::new(tr("settings.page_view.double_lr")),
                                        )
                                        .clicked();
                                    changed |= ui
                                        .radio_value(
                                            &mut self.config.page_view_options,
                                            PageViewOptions::TopDown,
                                            egui::RichText::new(tr("settings.page_view.top_down")),
                                        )
                                        .clicked();
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
                                        self.config.follow_chosen_page_view();
                                        self.reset_buffer();
                                        self.texture_cache.clear();
                                        self.reset_pan();
                                        if self.config.page_view_options == PageViewOptions::TopDown
                                        {
                                            self.textures = [None, None];
                                            self.top_down_textures =
                                                vec![None; self.image_files.len()];
                                            self.top_down_scroll_offset = 0.0;
                                            self.ensure_top_down_loaded_around(
                                                self.current_index,
                                                ctx,
                                            );
                                        } else {
                                            if previous_page_view == PageViewOptions::TopDown {
                                                self.top_down_textures.clear();
                                            }
                                            self.load_current_pair(ctx);
                                        }
                                        self.save_settings();
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
                                        self.reset_buffer();
                                        self.load_current_pair(ctx);
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
                                        ImageSizingMode::OriginalSize,
                                        egui::RichText::new(tr("settings.image_sizing.original")),
                                    )
                                    .clicked();
                                if image_sizing_changed {
                                    self.reset_buffer();
                                    self.texture_cache.clear();
                                    self.reset_pan();
                                    self.load_current_pair(ctx);
                                    self.save_settings();
                                }
                                separator_pct(ui);

                                ui.add_space(20.0);
                                ui.label(
                                    egui::RichText::new(tr("settings.others"))
                                        .size(20.0)
                                        .strong(),
                                );
                                separator_pct(ui);
                                let mut auto_layout_changed = ui
                                    .checkbox(
                                        &mut self.config.auto_page_layout_rl,
                                        tr("settings.auto_page_layout_rl"),
                                    )
                                    .on_hover_text(tr("settings.auto_page_layout.tooltip"))
                                    .changed();
                                if auto_layout_changed && self.config.auto_page_layout_rl {
                                    self.config.auto_page_layout_lr = false;
                                    self.config.page_view_options = PageViewOptions::DoubleRL;
                                }
                                let lr_changed = ui
                                    .checkbox(
                                        &mut self.config.auto_page_layout_lr,
                                        tr("settings.auto_page_layout_lr"),
                                    )
                                    .on_hover_text(tr("settings.auto_page_layout.tooltip"))
                                    .changed();
                                if lr_changed && self.config.auto_page_layout_lr {
                                    self.config.auto_page_layout_rl = false;
                                    self.config.page_view_options = PageViewOptions::DoubleLR;
                                }
                                auto_layout_changed |= lr_changed;
                                if auto_layout_changed {
                                    self.reset_buffer();
                                    self.texture_cache.clear();
                                    self.clear_animated_image_cache();
                                    self.reset_pan();
                                    self.load_current_pair(ctx);
                                    self.save_settings();
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
                                // setting for auto hide the setting button
                                if ui
                                    .checkbox(
                                        &mut self.config.auto_hide_settings_button,
                                        tr("settings.auto_hide_settings_button"),
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
                                            &mut self.config.settings_button_width,
                                            3.0..=30.0,
                                        )
                                        .text(tr("settings.settings_button_width")),
                                    )
                                    .on_hover_text(tr("settings.settings_button_width.tooltip"))
                                    .changed()
                                {
                                    self.config.settings_button_width =
                                        self.config.settings_button_width.clamp(3.0, 30.0);
                                    self.save_settings();
                                }
                                ui.spacing_mut().slider_width = previous_slider_width;
                                let previous_slider_width = ui.spacing().slider_width;
                                ui.spacing_mut().slider_width =
                                    self.config.settings_width * 0.9 - 190.0;
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.config.settings_button_x_offset,
                                            0.0..=20.0,
                                        )
                                        .text(tr("settings.settings_button_x_position")),
                                    )
                                    .on_hover_text(tr(
                                        "settings.settings_button_x_position.tooltip",
                                    ))
                                    .changed()
                                {
                                    self.config.settings_button_x_offset =
                                        self.config.settings_button_x_offset.clamp(0.0, 20.0);
                                    self.save_settings();
                                }
                                ui.spacing_mut().slider_width = previous_slider_width;
                                let previous_slider_width = ui.spacing().slider_width;
                                ui.spacing_mut().slider_width =
                                    self.config.settings_width * 0.9 - 190.0;
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.config.settings_button_y_offset,
                                            -1.0..=1.0,
                                        )
                                        .text(tr("settings.settings_button_y_position")),
                                    )
                                    .on_hover_text(tr(
                                        "settings.settings_button_y_position.tooltip",
                                    ))
                                    .changed()
                                {
                                    self.config.settings_button_y_offset =
                                        self.config.settings_button_y_offset.clamp(-1.0, 1.0);
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

                                ui.add_space(50.0);
                            });
                        });
                });
        }
    }

    pub(super) fn render_settings_toggle(&mut self, ctx: &egui::Context) {
        // This allows opening/closing the settings
        let screen_rect = ctx.content_rect();
        let button_width = self.config.settings_button_width.clamp(3.0, 30.0);
        let button_height = 140.0;

        // Calculate X position based on whether panel is open
        let base_x_pos = if self.config.show_settings {
            screen_rect.max.x - self.config.settings_width - button_width - 20.0
        } else {
            screen_rect.max.x
        };
        let x_pos = base_x_pos + self.config.settings_button_x_offset.clamp(0.0, 20.0);

        // Calculate Y position to center the 200px button vertically
        let centered_y_pos = screen_rect.center().y - (button_height / 2.0);
        let min_y_pos = screen_rect.min.y;
        let max_y_pos = screen_rect.max.y - button_height;
        let y_pos =
            centered_y_pos + self.config.settings_button_y_offset * ((max_y_pos - min_y_pos) / 2.0);
        let y_pos = y_pos.clamp(min_y_pos, max_y_pos);

        // check whether we should auto hide the setting button
        let settings_toggle_reveal_width = 60.0
            + if self.config.show_settings {
                self.config.settings_width
            } else {
                0.0
            };
        let is_pointer_near_right_edge = ctx
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|pos| screen_rect.max.x - pos.x <= settings_toggle_reveal_width);
        let settings_toggle_pinned = self.config.show_settings
            || !self.config.auto_hide_settings_button
            || self.image_files.is_empty();
        let should_refresh_settings_toggle = settings_toggle_pinned || is_pointer_near_right_edge;
        if should_refresh_settings_toggle {
            self.settings_toggle_last_visible_at = Instant::now();
        }
        let fade_elapsed =
            Instant::now().saturating_duration_since(self.settings_toggle_last_visible_at);
        let settings_toggle_opacity = if should_refresh_settings_toggle {
            1.0
        } else {
            (1.0 - fade_elapsed.as_secs_f32() / SETTINGS_TOGGLE_FADE_DURATION.as_secs_f32())
                .clamp(0.0, 1.0)
        };

        if settings_toggle_opacity > 0.0 {
            if settings_toggle_opacity < 1.0 {
                ctx.request_repaint();
            }
            egui::Area::new(egui::Id::new("settings_toggle"))
                .fixed_pos([x_pos, y_pos])
                .show(ctx, |ui| {
                    let text = if self.config.show_settings {
                        "▶"
                    } else {
                        "◀"
                    };

                    ui.scope(|ui| {
                        let mut visuals = ui.style().visuals.clone();
                        visuals.widgets.inactive.bg_fill = visuals
                            .widgets
                            .inactive
                            .bg_fill
                            .gamma_multiply(settings_toggle_opacity);
                        visuals.widgets.inactive.weak_bg_fill = visuals
                            .widgets
                            .inactive
                            .weak_bg_fill
                            .gamma_multiply(settings_toggle_opacity);
                        visuals.widgets.inactive.bg_stroke.color = visuals
                            .widgets
                            .inactive
                            .bg_stroke
                            .color
                            .gamma_multiply(settings_toggle_opacity);
                        visuals.widgets.inactive.fg_stroke.color = visuals
                            .widgets
                            .inactive
                            .fg_stroke
                            .color
                            .gamma_multiply(settings_toggle_opacity);
                        visuals.override_text_color = Some(
                            ui.visuals()
                                .text_color()
                                .gamma_multiply(settings_toggle_opacity),
                        );
                        ui.style_mut().visuals = visuals;

                        let toggle_btn = egui::Button::new(
                            egui::RichText::new(text)
                                .size(if button_width < 16.0 { 0.0 } else { 16.0 })
                                .color(ui.visuals().text_color()),
                        );
                        if ui
                            .add_sized([button_width, button_height], toggle_btn)
                            .clicked()
                        {
                            self.config.show_settings = !self.config.show_settings;
                            self.settings_toggle_last_visible_at = Instant::now();
                        }
                    });
                });
        }
    }
}
