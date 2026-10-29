// このファイルは RustMangaReader（Lycoris52、Apache License 2.0）のコードを元に改変したものです。

use super::settings_panel::{BindingTarget, ControlProfile};
use super::thumbnails::ShelfSource;
use super::{GAMEPAD_INITIAL_REPEAT_DELAY, LONG_CLICK_DURATION, MOUSE_DRAG_THRESHOLD, MangaReader};
use crate::config::{
    GamepadButton, MangaAction, MouseButton, MouseConfig, MouseGesture, PageViewOptions, Shortcut,
};
use crate::engine::thumbnail::ThumbnailMode;
use crate::utils::{
    gamepad_action_for_button, gamepad_button_index, is_repeatable_gamepad_action,
    map_gamepad_button, mouse_button_index, mouse_button_to_pointer,
};
use eframe::egui;
use gilrs::EventType;
use std::time::{Duration, Instant};

impl MangaReader {
    pub(super) fn collect_input_action(&mut self, ctx: &egui::Context) -> MangaAction {
        let mut action_to_run = MangaAction::None;

        // REBINDING LOGIC
        if let Some(binding_target) = self.binding_action.clone() {
            match binding_target {
                BindingTarget::Keyboard { profile, action } => {
                    ctx.input(|i| {
                        for key in egui::Key::ALL {
                            if i.key_pressed(*key) {
                                let new_shortcut = Shortcut {
                                    key: *key,
                                    ctrl: i.modifiers.ctrl,
                                    alt: i.modifiers.alt,
                                    shift: i.modifiers.shift,
                                };

                                let keys = match profile {
                                    ControlProfile::Default => &mut self.config.keys,
                                    ControlProfile::TopDown => &mut self.config.top_down_keys,
                                };
                                match action.as_str() {
                                    "Slide Image Down" => {
                                        keys.slide_image_down = Some(new_shortcut)
                                    }
                                    "Slide Image Up" => keys.slide_image_up = Some(new_shortcut),
                                    "Toggle Auto Scroll" => {
                                        keys.toggle_auto_scroll = Some(new_shortcut)
                                    }
                                    "Reload Current Image" => {
                                        keys.reload_current_image = Some(new_shortcut)
                                    }
                                    "Next Page" => keys.next_page = Some(new_shortcut),
                                    "Previous Page" => keys.prev_page = Some(new_shortcut),
                                    "1 Next Page" => keys.one_next_page = Some(new_shortcut),
                                    "1 Prev Page" => keys.one_prev_page = Some(new_shortcut),
                                    "First Page" => keys.first_page = Some(new_shortcut),
                                    "Last Page" => keys.last_page = Some(new_shortcut),
                                    "Next File" => keys.next_file = Some(new_shortcut),
                                    "Previous File" => keys.prev_file = Some(new_shortcut),
                                    "Next Folder" => keys.next_folder = Some(new_shortcut),
                                    "Previous Folder" => keys.prev_folder = Some(new_shortcut),
                                    "Toggle Fullscreen" => keys.fullscreen = Some(new_shortcut),
                                    "View Mode" => keys.view_mode = Some(new_shortcut),
                                    "Open File" => keys.open_file = Some(new_shortcut),
                                    "Quit App" => keys.quit_app = Some(new_shortcut),
                                    "Cycle Last Page Action" => {
                                        keys.cycle_last_page_action = Some(new_shortcut)
                                    }
                                    "Scroll Next Page" => {
                                        keys.scroll_next_page = Some(new_shortcut)
                                    }
                                    "Scroll Prev Page" => {
                                        keys.scroll_prev_page = Some(new_shortcut)
                                    }
                                    "Scroll Up" => keys.scroll_up = Some(new_shortcut),
                                    "Scroll Down" => keys.scroll_down = Some(new_shortcut),
                                    "Scroll Left" => keys.scroll_left = Some(new_shortcut),
                                    "Scroll Right" => keys.scroll_right = Some(new_shortcut),
                                    "Toggle Loupe" => keys.toggle_loupe = Some(new_shortcut),
                                    "Toggle Auto Page Layout" => {
                                        keys.toggle_auto_page_layout = Some(new_shortcut)
                                    }
                                    "Jump Forward" => keys.jump_forward = Some(new_shortcut),
                                    "Jump Backward" => keys.jump_backward = Some(new_shortcut),
                                    "Toggle Split Pages" => {
                                        keys.toggle_split_pages = Some(new_shortcut)
                                    }
                                    "Thumbnails Fit All" => {
                                        keys.thumbnails_fit_all = Some(new_shortcut)
                                    }
                                    "Thumbnails Large" => {
                                        keys.thumbnails_large = Some(new_shortcut)
                                    }
                                    "Cover Thumbnails" => {
                                        keys.cover_thumbnails = Some(new_shortcut)
                                    }
                                    "Upper Folder" => keys.upper_folder = Some(new_shortcut),
                                    "Show History" => keys.show_history = Some(new_shortcut),
                                    "Show Favorites" => keys.show_favorites = Some(new_shortcut),
                                    "Toggle Favorite" => keys.toggle_favorite = Some(new_shortcut),
                                    _ => {}
                                }
                                self.binding_action = None;
                                self.save_settings();
                            }
                        }
                    });
                }
            }
        }
        // PART B: EXECUTION LOGIC
        else {
            ctx.input(|i| {
                let keys = if self.config.page_view_options == PageViewOptions::TopDown {
                    self.config.top_down_keys
                } else {
                    self.config.keys
                };

                // Helper to check if a shortcut is triggered
                let is_triggered = |s: &Option<Shortcut>| {
                    s.is_some_and(|s| {
                        i.key_pressed(s.key)
                            && i.modifiers.ctrl == s.ctrl
                            && i.modifiers.alt == s.alt
                            && i.modifiers.shift == s.shift
                    })
                };

                if is_triggered(&keys.slide_image_down) {
                    action_to_run = MangaAction::SlideImageDown;
                }
                if is_triggered(&keys.slide_image_up) {
                    action_to_run = MangaAction::SlideImageUp;
                }
                if is_triggered(&keys.toggle_auto_scroll) {
                    action_to_run = MangaAction::ToggleAutoScroll;
                }
                if is_triggered(&keys.reload_current_image) {
                    action_to_run = MangaAction::ReloadCurrentImage;
                }
                if is_triggered(&keys.next_page) {
                    action_to_run = MangaAction::NextPage;
                }
                if is_triggered(&keys.prev_page) {
                    action_to_run = MangaAction::PrevPage;
                }
                if is_triggered(&keys.one_next_page) {
                    action_to_run = MangaAction::OneNextPage;
                }
                if is_triggered(&keys.one_prev_page) {
                    action_to_run = MangaAction::OnePrevPage;
                }
                if is_triggered(&keys.first_page) {
                    action_to_run = MangaAction::FirstPage;
                }
                if is_triggered(&keys.last_page) {
                    action_to_run = MangaAction::LastPage;
                }
                if is_triggered(&keys.next_file) {
                    action_to_run = MangaAction::NextFile;
                }
                if is_triggered(&keys.prev_file) {
                    action_to_run = MangaAction::PrevFile;
                }
                if is_triggered(&keys.next_folder) {
                    action_to_run = MangaAction::NextFolder;
                }
                if is_triggered(&keys.prev_folder) {
                    action_to_run = MangaAction::PrevFolder;
                }
                if is_triggered(&keys.fullscreen) {
                    action_to_run = MangaAction::FullScreen;
                }
                if is_triggered(&keys.view_mode) {
                    action_to_run = MangaAction::ViewMode;
                }
                if is_triggered(&keys.open_file) {
                    action_to_run = MangaAction::OpenFile;
                }
                if is_triggered(&keys.quit_app) {
                    action_to_run = MangaAction::QuitApp;
                }
                if is_triggered(&keys.cycle_last_page_action) {
                    action_to_run = MangaAction::CycleLastPageAction;
                }
                if is_triggered(&keys.scroll_next_page) {
                    action_to_run = MangaAction::ScrollNextPage;
                }
                if is_triggered(&keys.scroll_prev_page) {
                    action_to_run = MangaAction::ScrollPrevPage;
                }
                if is_triggered(&keys.scroll_up) {
                    action_to_run = MangaAction::ScrollUp;
                }
                if is_triggered(&keys.scroll_down) {
                    action_to_run = MangaAction::ScrollDown;
                }
                if is_triggered(&keys.scroll_left) {
                    action_to_run = MangaAction::ScrollLeft;
                }
                if is_triggered(&keys.scroll_right) {
                    action_to_run = MangaAction::ScrollRight;
                }
                if is_triggered(&keys.toggle_loupe) {
                    action_to_run = MangaAction::ToggleLoupe;
                }
                if is_triggered(&keys.toggle_auto_page_layout) {
                    action_to_run = MangaAction::ToggleAutoPageLayout;
                }
                if is_triggered(&keys.jump_forward) {
                    action_to_run = MangaAction::JumpForward;
                }
                if is_triggered(&keys.jump_backward) {
                    action_to_run = MangaAction::JumpBackward;
                }
                if is_triggered(&keys.toggle_split_pages) {
                    action_to_run = MangaAction::ToggleSplitPages;
                }
                if is_triggered(&keys.thumbnails_fit_all) {
                    action_to_run = MangaAction::ThumbnailsFitAll;
                }
                if is_triggered(&keys.thumbnails_large) {
                    action_to_run = MangaAction::ThumbnailsLarge;
                }
                if is_triggered(&keys.cover_thumbnails) {
                    action_to_run = MangaAction::CoverThumbnails;
                }
                if is_triggered(&keys.upper_folder) {
                    action_to_run = MangaAction::UpperFolder;
                }
                if is_triggered(&keys.show_history) {
                    action_to_run = MangaAction::ShowHistory;
                }
                if is_triggered(&keys.show_favorites) {
                    action_to_run = MangaAction::ShowFavorites;
                }
                if is_triggered(&keys.toggle_favorite) {
                    action_to_run = MangaAction::ToggleFavorite;
                }
            });
        }

        if self.binding_action.is_none() && action_to_run == MangaAction::None {
            if let Some(gamepad_action) = self.collect_gamepad_action(ctx) {
                action_to_run = gamepad_action;
            }
        }

        action_to_run
    }

    pub(super) fn current_mouse_config(&self) -> MouseConfig {
        if self.config.page_view_options == PageViewOptions::TopDown {
            self.config.top_down_mouse
        } else {
            self.config.mouse
        }
    }

    /// `clicked` が他のボタンを押したままクリックされ、その組み合わせに操作があれば返す
    fn chord_click(
        &self,
        clicked: MouseButton,
        ctx: &egui::Context,
    ) -> Option<(MouseButton, MangaAction)> {
        let mouse = self.current_mouse_config();
        MouseButton::ALL
            .into_iter()
            .filter(|&held| held != clicked)
            .filter(|&held| ctx.input(|i| i.pointer.button_down(mouse_button_to_pointer(held))))
            .map(|held| (held, mouse.chord_click_action(held, clicked)))
            .find(|&(_, action)| action != MangaAction::None)
    }

    /// 押したままホイールを回すと操作が割り当てられているボタンのうち、押されているもの
    fn held_chord_scroll_button(&self, ctx: &egui::Context) -> Option<MouseButton> {
        let mouse = self.current_mouse_config();
        MouseButton::ALL.into_iter().find(|&held| {
            (mouse.chord_scroll_action(held, true) != MangaAction::None
                || mouse.chord_scroll_action(held, false) != MangaAction::None)
                && ctx.input(|i| i.pointer.button_down(mouse_button_to_pointer(held)))
        })
    }

    pub(super) fn get_mouse_action(&self, gesture: MouseGesture) -> MangaAction {
        let mouse = self.current_mouse_config();
        match gesture {
            MouseGesture::Unassigned => MangaAction::None,
            MouseGesture::ScrollUp => mouse.scroll_up,
            MouseGesture::ScrollDown => mouse.scroll_down,
            MouseGesture::Click(MouseButton::Button1) => mouse.button1_click,
            MouseGesture::Click(MouseButton::Button2) => mouse.button2_click,
            MouseGesture::Click(MouseButton::Button3) => mouse.button3_click,
            MouseGesture::Click(MouseButton::Button4) => mouse.button4_click,
            MouseGesture::Click(MouseButton::Button5) => mouse.button5_click,
            MouseGesture::DoubleClick(MouseButton::Button1) => mouse.button1_double_click,
            MouseGesture::DoubleClick(MouseButton::Button2) => mouse.button2_double_click,
            MouseGesture::DoubleClick(MouseButton::Button3) => mouse.button3_double_click,
            MouseGesture::DoubleClick(MouseButton::Button4) => mouse.button4_double_click,
            MouseGesture::DoubleClick(MouseButton::Button5) => mouse.button5_double_click,
            MouseGesture::LongClick(MouseButton::Button1) => mouse.button1_long_click,
            MouseGesture::LongClick(MouseButton::Button2) => mouse.button2_long_click,
            MouseGesture::LongClick(MouseButton::Button3) => mouse.button3_long_click,
            MouseGesture::LongClick(MouseButton::Button4) => mouse.button4_long_click,
            MouseGesture::LongClick(MouseButton::Button5) => mouse.button5_long_click,
        }
    }

    pub(super) fn gamepad_repeat_interval(&self) -> Duration {
        Duration::from_millis(self.config.image_delay.max(80))
    }

    pub(super) fn collect_gamepad_action(&mut self, ctx: &egui::Context) -> Option<MangaAction> {
        let gamepad_config = if self.config.page_view_options == PageViewOptions::TopDown {
            self.config.top_down_gamepad
        } else {
            self.config.gamepad
        };
        let repeat_interval = self.gamepad_repeat_interval();
        let gilrs = self.gamepad.as_mut()?;
        let mut action_to_run = None;
        let now = Instant::now();

        while let Some(event) = gilrs.next_event() {
            // ゲームパッドの操作も、操作したことにする（裏での下ごしらえを止める）
            self.text_prefetch.touched();
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(mapped_button) = map_gamepad_button(button) {
                        let action = gamepad_action_for_button(gamepad_config, mapped_button);
                        let index = gamepad_button_index(mapped_button);
                        if is_repeatable_gamepad_action(action) {
                            self.gamepad_repeat_deadlines[index] =
                                Some(now + GAMEPAD_INITIAL_REPEAT_DELAY);
                            ctx.request_repaint_after(GAMEPAD_INITIAL_REPEAT_DELAY);
                        } else {
                            self.gamepad_repeat_deadlines[index] = None;
                        }
                        if action != MangaAction::None {
                            action_to_run = Some(action);
                        }
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(mapped_button) = map_gamepad_button(button) {
                        let index = gamepad_button_index(mapped_button);
                        self.gamepad_repeat_deadlines[index] = None;
                    }
                }
                _ => {}
            }
        }

        let now = Instant::now();
        for button in GamepadButton::ALL {
            let index = gamepad_button_index(button);
            if let Some(deadline) = self.gamepad_repeat_deadlines[index] {
                if now >= deadline {
                    let action = gamepad_action_for_button(gamepad_config, button);
                    if is_repeatable_gamepad_action(action) {
                        self.gamepad_repeat_deadlines[index] = Some(now + repeat_interval);
                        ctx.request_repaint_after(repeat_interval);
                        if action != MangaAction::None {
                            action_to_run = Some(action);
                        }
                    } else {
                        self.gamepad_repeat_deadlines[index] = None;
                    }
                } else {
                    ctx.request_repaint_after(deadline - now);
                }
            }
        }

        action_to_run
    }

    pub(super) fn double_click_threshold(&self) -> Duration {
        let threshold_ms = if self.config.page_view_options == PageViewOptions::TopDown {
            self.config.top_down_double_click_threshold_ms
        } else {
            self.config.double_click_threshold_ms
        };
        Duration::from_millis(threshold_ms)
    }

    pub(super) fn collect_pending_mouse_click_action(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<MangaAction> {
        let now = Instant::now();
        let threshold = self.double_click_threshold();

        for pending in &self.pending_mouse_click {
            if let Some((deadline, _)) = pending {
                if *deadline > now {
                    ctx.request_repaint_after(*deadline - now);
                }
            }
        }

        for pending in &mut self.pending_mouse_click {
            if let Some((deadline, action)) = *pending {
                if now >= deadline {
                    *pending = None;
                    if action != MangaAction::None {
                        return Some(action);
                    }
                } else if deadline.duration_since(now) <= threshold {
                    ctx.request_repaint();
                }
            }
        }

        None
    }

    pub(super) fn collect_mouse_action(
        &mut self,
        response: &egui::Response,
        ctx: &egui::Context,
    ) -> Option<MangaAction> {
        if let Some(action) = self.collect_pending_mouse_click_action(ctx) {
            return Some(action);
        }

        if !response.hovered() {
            for button in MouseButton::ALL {
                let index = mouse_button_index(button);
                if !ctx.input(|i| i.pointer.button_down(mouse_button_to_pointer(button))) {
                    self.mouse_press_started[index] = None;
                    self.mouse_press_origin[index] = None;
                    self.mouse_long_triggered[index] = false;
                    self.mouse_drag_suppressed[index] = false;
                    self.mouse_chord_used[index] = false;
                }
            }
            return None;
        }

        for button in MouseButton::ALL {
            let pointer_button = mouse_button_to_pointer(button);
            let button_index = mouse_button_index(button);
            let now = Instant::now();
            let is_down = ctx.input(|i| i.pointer.button_down(pointer_button));

            if is_down {
                if self.mouse_press_started[button_index].is_none() {
                    self.mouse_press_started[button_index] = Some(now);
                    self.mouse_press_origin[button_index] = ctx.input(|i| i.pointer.interact_pos());
                    self.mouse_long_triggered[button_index] = false;
                    self.mouse_drag_suppressed[button_index] = false;
                    ctx.request_repaint();
                } else if !self.mouse_drag_suppressed[button_index] {
                    if let (Some(origin), Some(current)) = (
                        self.mouse_press_origin[button_index],
                        ctx.input(|i| i.pointer.interact_pos()),
                    ) {
                        if origin.distance(current) > MOUSE_DRAG_THRESHOLD {
                            self.mouse_drag_suppressed[button_index] = true;
                            self.pending_mouse_click[button_index] = None;
                        }
                    }
                }

                if !self.mouse_drag_suppressed[button_index]
                    && !self.mouse_long_triggered[button_index]
                    && !self.mouse_chord_used[button_index]
                    && self.mouse_press_started[button_index]
                        .map(|started| now.duration_since(started) >= LONG_CLICK_DURATION)
                        .unwrap_or(false)
                {
                    self.mouse_long_triggered[button_index] = true;
                    self.pending_mouse_click[button_index] = None;
                    let action = self.get_mouse_action(MouseGesture::LongClick(button));
                    if action != MangaAction::None {
                        return Some(action);
                    }
                } else if !self.mouse_long_triggered[button_index]
                    && !self.mouse_drag_suppressed[button_index]
                {
                    ctx.request_repaint();
                }
            }

            if response.clicked_by(pointer_button) {
                // 他のボタンを押したままのクリック（同時押し）
                if let Some((held, action)) = self.chord_click(button, ctx) {
                    self.mouse_chord_used[mouse_button_index(held)] = true;
                    self.pending_mouse_click[button_index] = None;
                    return Some(action);
                }
            }

            let was_long_click = self.mouse_long_triggered[button_index];
            let was_dragging = self.mouse_drag_suppressed[button_index];
            let was_chord = self.mouse_chord_used[button_index];
            if response.clicked_by(pointer_button) && was_chord {
                // 同時押しに使ったボタンを離しただけなので、クリックとしては扱わない
                self.mouse_chord_used[button_index] = false;
                self.pending_mouse_click[button_index] = None;
            } else if response.clicked_by(pointer_button) && !was_long_click && !was_dragging {
                self.mouse_press_started[button_index] = None;
                self.mouse_press_origin[button_index] = None;
                self.mouse_long_triggered[button_index] = false;
                self.mouse_drag_suppressed[button_index] = false;
                let now = Instant::now();
                let double_action = self.get_mouse_action(MouseGesture::DoubleClick(button));
                let click_action = self.get_mouse_action(MouseGesture::Click(button));

                if let Some((deadline, _)) = self.pending_mouse_click[button_index] {
                    if now <= deadline {
                        self.pending_mouse_click[button_index] = None;
                        if double_action != MangaAction::None {
                            return Some(double_action);
                        }
                        if click_action != MangaAction::None {
                            return Some(click_action);
                        }
                    } else {
                        self.pending_mouse_click[button_index] = None;
                    }
                } else if double_action != MangaAction::None {
                    self.pending_mouse_click[button_index] =
                        Some((now + self.double_click_threshold(), click_action));
                    ctx.request_repaint_after(self.double_click_threshold());
                } else if click_action != MangaAction::None {
                    return Some(click_action);
                }
            }

            if !is_down {
                self.mouse_press_started[button_index] = None;
                self.mouse_press_origin[button_index] = None;
                self.mouse_long_triggered[button_index] = false;
                self.mouse_drag_suppressed[button_index] = false;
                self.mouse_chord_used[button_index] = false;
            }
        }

        let scroll_delta = ctx.input(|i| i.smooth_scroll_delta);
        let scroll_threshold = 2.0;

        // ボタンを押したままのホイール（同時押し）。1回のホイール操作につき1回だけ実行する
        if let Some(held) = self.held_chord_scroll_button(ctx) {
            if self.can_scroll {
                let up = scroll_delta.y > scroll_threshold || scroll_delta.x > scroll_threshold;
                let down = scroll_delta.y < -scroll_threshold || scroll_delta.x < -scroll_threshold;
                if up || down {
                    self.can_scroll = false;
                    self.mouse_chord_used[mouse_button_index(held)] = true;
                    let action = self.current_mouse_config().chord_scroll_action(held, up);
                    return Some(action);
                }
            } else if scroll_delta.y.abs() <= scroll_threshold
                && scroll_delta.x.abs() <= scroll_threshold
            {
                self.can_scroll = true;
            }
            return None;
        }
        if self.config.page_view_options == PageViewOptions::TopDown {
            if scroll_delta.y > scroll_threshold || scroll_delta.x > scroll_threshold {
                let action = self.get_mouse_action(MouseGesture::ScrollUp);
                if action != MangaAction::None {
                    return Some(action);
                }
            } else if scroll_delta.y < -scroll_threshold || scroll_delta.x < -scroll_threshold {
                let action = self.get_mouse_action(MouseGesture::ScrollDown);
                if action != MangaAction::None {
                    return Some(action);
                }
            }
            return None;
        }

        if self.can_scroll {
            if scroll_delta.y > scroll_threshold || scroll_delta.x > scroll_threshold {
                self.can_scroll = false;
                let action = self.get_mouse_action(MouseGesture::ScrollUp);
                if action != MangaAction::None {
                    return Some(action);
                }
            } else if scroll_delta.y < -scroll_threshold || scroll_delta.x < -scroll_threshold {
                self.can_scroll = false;
                let action = self.get_mouse_action(MouseGesture::ScrollDown);
                if action != MangaAction::None {
                    return Some(action);
                }
            }
        } else if scroll_delta.y.abs() <= scroll_threshold
            && scroll_delta.x.abs() <= scroll_threshold
        {
            self.can_scroll = true;
        }

        None
    }

    pub(super) fn execute_action(&mut self, action: MangaAction, ctx: &egui::Context) {
        match action {
            MangaAction::ScrollNextPage => self.scroll_next_page(ctx),
            MangaAction::ScrollPrevPage => self.scroll_prev_page(ctx),
            MangaAction::ScrollUp => self.scroll_view(0.0, -1.0, ctx),
            MangaAction::ScrollDown => self.scroll_view(0.0, 1.0, ctx),
            MangaAction::ScrollLeft => self.scroll_view(-1.0, 0.0, ctx),
            MangaAction::ScrollRight => self.scroll_view(1.0, 0.0, ctx),
            MangaAction::ToggleLoupe => self.set_loupe_enabled(!self.loupe_enabled, ctx),
            MangaAction::ToggleAutoPageLayout => self.toggle_auto_page_layout(ctx),
            MangaAction::JumpForward => self.jump_pages(true, ctx),
            MangaAction::JumpBackward => self.jump_pages(false, ctx),
            MangaAction::ToggleSplitPages => self.toggle_split_landscape_pages(ctx),
            MangaAction::ThumbnailsFitAll => self.open_thumbnails(ThumbnailMode::FitAll),
            MangaAction::ThumbnailsLarge => self.open_thumbnails(ThumbnailMode::Large),
            MangaAction::CoverThumbnails => self.toggle_cover_thumbnails(),
            MangaAction::UpperFolder => self.go_to_upper_folder(),
            MangaAction::ShowHistory => self.toggle_book_list(ShelfSource::History),
            MangaAction::ShowFavorites => self.toggle_book_list(ShelfSource::Favorites),
            MangaAction::ToggleFavorite => self.toggle_current_favorite(),
            MangaAction::SlideImageDown => {
                if self.config.page_view_options == PageViewOptions::TopDown {
                    self.top_down_scroll_offset -=
                        ctx.content_rect().height() * self.config.top_down_image_slide_speed;
                    self.clamp_top_down_scroll_offset();
                }
            }
            MangaAction::SlideImageUp => {
                if self.config.page_view_options == PageViewOptions::TopDown {
                    self.top_down_scroll_offset +=
                        ctx.content_rect().height() * self.config.top_down_image_slide_speed;
                    self.clamp_top_down_scroll_offset();
                }
            }
            MangaAction::ToggleAutoScroll => self.toggle_auto_scroll(ctx),
            MangaAction::ReloadCurrentImage => self.reload_current_image(ctx),
            MangaAction::ZoomIn => self.zoom_in(ctx),
            MangaAction::ZoomOut => self.zoom_out(ctx),
            MangaAction::NextPage => self.next_page(ctx, false),
            MangaAction::PrevPage => self.prev_page(ctx, false),
            MangaAction::OneNextPage => self.next_page(ctx, true),
            MangaAction::OnePrevPage => self.prev_page(ctx, true),
            MangaAction::FirstPage => self.go_to_first_page(ctx),
            MangaAction::LastPage => self.go_to_last_page(ctx),
            MangaAction::NextFile => self.next_zip(ctx),
            MangaAction::PrevFile => self.prev_zip(ctx, false),
            MangaAction::NextFolder => self.next_folder(ctx),
            MangaAction::PrevFolder => self.prev_folder(ctx),
            MangaAction::FullScreen => {
                self.toggle_fullscreen(ctx);
            }
            MangaAction::ViewMode => self.change_shifted_mode(ctx),
            MangaAction::OpenFile => self.open_file_dialog(),
            MangaAction::QuitApp => {
                self.save_settings();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            MangaAction::CycleLastPageAction => self.cycle_last_page_action(),
            MangaAction::None => {}
        }
    }
}
