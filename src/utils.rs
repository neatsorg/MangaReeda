use crate::config::{GamepadButton, GamepadConfig, MangaAction, MouseButton, UiLanguage};
use crate::localize::tr;
use egui::PointerButton;
use gilrs::Button as GilrsButton;

pub fn separator_pct(ui: &mut egui::Ui) {
    let pct = 0.9;

    let spacing = ui.spacing();
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;

    // Allocate a thin horizontal area (like separator does)
    let desired_h = spacing.scroll.bar_width; // this is usually ~1.0
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), desired_h),
        egui::Sense::hover(),
    );

    // Compute a centered line that's pct of the available width
    let full_w = rect.width();
    let line_w = full_w * pct;

    let indent = 0.0;
    let x0 = rect.left() + indent;
    let x1 = (x0 + line_w).min(rect.right());
    let y = rect.center().y;

    ui.painter()
        .line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], stroke);
}

pub fn render_mouse_action_dropdown(
    ui: &mut egui::Ui,
    id: &str,
    action: &mut MangaAction,
    action_options: &[(MangaAction, String)],
    unassigned_label: &str,
) -> bool {
    let previous = *action;
    egui::ComboBox::from_id_salt(id)
        .selected_text(
            action_options
                .iter()
                .find(|(option, _)| option == action)
                .map(|(_, label)| label.as_str())
                .unwrap_or(unassigned_label),
        )
        .width(150.0)
        .show_ui(ui, |ui| {
            for (option, label) in action_options {
                ui.selectable_value(action, *option, label);
            }
        });

    if *action != previous {
        ui.ctx().request_repaint();
        true
    } else {
        false
    }
}

pub fn language_label(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::English => tr("language.english"),
        UiLanguage::Japanese => tr("language.japanese"),
    }
}

pub fn action_label(action: MangaAction) -> &'static str {
    match action {
        MangaAction::None => tr("action.none"),
        MangaAction::SlideImageDown => tr("action.slide_image_down"),
        MangaAction::SlideImageUp => tr("action.slide_image_up"),
        MangaAction::ToggleAutoScroll => tr("action.toggle_auto_scroll"),
        MangaAction::ReloadCurrentImage => tr("action.reload_current_image"),
        MangaAction::ZoomIn => tr("action.zoom_in"),
        MangaAction::ZoomOut => tr("action.zoom_out"),
        MangaAction::NextPage => tr("action.next_page"),
        MangaAction::PrevPage => tr("action.prev_page"),
        MangaAction::OneNextPage => tr("action.one_next_page"),
        MangaAction::OnePrevPage => tr("action.one_prev_page"),
        MangaAction::FirstPage => tr("action.first_page"),
        MangaAction::LastPage => tr("action.last_page"),
        MangaAction::NextFile => tr("action.next_file"),
        MangaAction::PrevFile => tr("action.prev_file"),
        MangaAction::NextFolder => tr("action.next_folder"),
        MangaAction::PrevFolder => tr("action.prev_folder"),
        MangaAction::FullScreen => tr("action.fullscreen"),
        MangaAction::ViewMode => tr("action.view_mode"),
        MangaAction::OpenFile => tr("action.open_file"),
        MangaAction::QuitApp => tr("action.quit_app"),
    }
}

pub fn gamepad_button_label(button: GamepadButton) -> &'static str {
    match button {
        GamepadButton::South => tr("settings.gamepad.south"),
        GamepadButton::East => tr("settings.gamepad.east"),
        GamepadButton::North => tr("settings.gamepad.north"),
        GamepadButton::West => tr("settings.gamepad.west"),
        GamepadButton::LeftTrigger => tr("settings.gamepad.left_trigger"),
        GamepadButton::LeftTrigger2 => tr("settings.gamepad.left_trigger2"),
        GamepadButton::RightTrigger => tr("settings.gamepad.right_trigger"),
        GamepadButton::RightTrigger2 => tr("settings.gamepad.right_trigger2"),
        GamepadButton::Select => tr("settings.gamepad.select"),
        GamepadButton::Start => tr("settings.gamepad.start"),
        GamepadButton::LeftThumb => tr("settings.gamepad.left_thumb"),
        GamepadButton::RightThumb => tr("settings.gamepad.right_thumb"),
        GamepadButton::DPadUp => tr("settings.gamepad.dpad_up"),
        GamepadButton::DPadDown => tr("settings.gamepad.dpad_down"),
        GamepadButton::DPadLeft => tr("settings.gamepad.dpad_left"),
        GamepadButton::DPadRight => tr("settings.gamepad.dpad_right"),
    }
}

pub fn mouse_button_to_pointer(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Button1 => PointerButton::Primary,
        MouseButton::Button2 => PointerButton::Secondary,
        MouseButton::Button3 => PointerButton::Middle,
        MouseButton::Button4 => PointerButton::Extra1,
        MouseButton::Button5 => PointerButton::Extra2,
    }
}

pub fn mouse_button_index(button: MouseButton) -> usize {
    match button {
        MouseButton::Button1 => 0,
        MouseButton::Button2 => 1,
        MouseButton::Button3 => 2,
        MouseButton::Button4 => 3,
        MouseButton::Button5 => 4,
    }
}

pub fn map_gamepad_button(button: GilrsButton) -> Option<GamepadButton> {
    match button {
        GilrsButton::South => Some(GamepadButton::South),
        GilrsButton::East => Some(GamepadButton::East),
        GilrsButton::North => Some(GamepadButton::North),
        GilrsButton::West => Some(GamepadButton::West),
        GilrsButton::LeftTrigger => Some(GamepadButton::LeftTrigger),
        GilrsButton::LeftTrigger2 => Some(GamepadButton::LeftTrigger2),
        GilrsButton::RightTrigger => Some(GamepadButton::RightTrigger),
        GilrsButton::RightTrigger2 => Some(GamepadButton::RightTrigger2),
        GilrsButton::Select => Some(GamepadButton::Select),
        GilrsButton::Start => Some(GamepadButton::Start),
        GilrsButton::LeftThumb => Some(GamepadButton::LeftThumb),
        GilrsButton::RightThumb => Some(GamepadButton::RightThumb),
        GilrsButton::DPadUp => Some(GamepadButton::DPadUp),
        GilrsButton::DPadDown => Some(GamepadButton::DPadDown),
        GilrsButton::DPadLeft => Some(GamepadButton::DPadLeft),
        GilrsButton::DPadRight => Some(GamepadButton::DPadRight),
        _ => None,
    }
}

pub fn gamepad_button_index(button: GamepadButton) -> usize {
    match button {
        GamepadButton::South => 0,
        GamepadButton::East => 1,
        GamepadButton::North => 2,
        GamepadButton::West => 3,
        GamepadButton::LeftTrigger => 4,
        GamepadButton::LeftTrigger2 => 5,
        GamepadButton::RightTrigger => 6,
        GamepadButton::RightTrigger2 => 7,
        GamepadButton::Select => 8,
        GamepadButton::Start => 9,
        GamepadButton::LeftThumb => 10,
        GamepadButton::RightThumb => 11,
        GamepadButton::DPadUp => 12,
        GamepadButton::DPadDown => 13,
        GamepadButton::DPadLeft => 14,
        GamepadButton::DPadRight => 15,
    }
}

pub fn gamepad_action_for_button(config: GamepadConfig, button: GamepadButton) -> MangaAction {
    match button {
        GamepadButton::South => config.south,
        GamepadButton::East => config.east,
        GamepadButton::North => config.north,
        GamepadButton::West => config.west,
        GamepadButton::LeftTrigger => config.left_trigger,
        GamepadButton::LeftTrigger2 => config.left_trigger2,
        GamepadButton::RightTrigger => config.right_trigger,
        GamepadButton::RightTrigger2 => config.right_trigger2,
        GamepadButton::Select => config.select,
        GamepadButton::Start => config.start,
        GamepadButton::LeftThumb => config.left_thumb,
        GamepadButton::RightThumb => config.right_thumb,
        GamepadButton::DPadUp => config.dpad_up,
        GamepadButton::DPadDown => config.dpad_down,
        GamepadButton::DPadLeft => config.dpad_left,
        GamepadButton::DPadRight => config.dpad_right,
    }
}

pub fn is_repeatable_gamepad_action(action: MangaAction) -> bool {
    matches!(
        action,
        MangaAction::NextPage
            | MangaAction::SlideImageDown
            | MangaAction::SlideImageUp
            | MangaAction::PrevPage
            | MangaAction::OneNextPage
            | MangaAction::OnePrevPage
            | MangaAction::ZoomIn
            | MangaAction::ZoomOut
            | MangaAction::NextFile
            | MangaAction::PrevFile
            | MangaAction::NextFolder
            | MangaAction::PrevFolder
    )
}
