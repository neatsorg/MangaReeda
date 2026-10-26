mod animation;
mod input;
mod loader;
mod navigation;
mod overlay;
mod pageview;
mod settings_panel;
mod toolbar;
mod view;

use crate::config::{AppSettings, MangaAction, SourceMode};
use crate::engine::settings_store;
use crate::font;
use crate::localize::{set_language, tr};
use animation::{AnimatedFrame, AnimatedPage, AnimationScanEvent, AnimationScanStatus};
use eframe::egui;
use gilrs::Gilrs;
use pdfium_render::prelude::{PdfDocument, Pdfium};
use settings_panel::{BindingTarget, ControlProfile};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

const LONG_CLICK_DURATION: Duration = Duration::from_millis(450);
const GAMEPAD_INITIAL_REPEAT_DELAY: Duration = Duration::from_millis(800);
const MOUSE_DRAG_THRESHOLD: f32 = 6.0;
const SETTINGS_TOGGLE_FADE_DURATION: Duration = Duration::from_secs(1);
const AUTO_SCROLL_MIN_DELAY_MS: u64 = 200;
const AUTO_SCROLL_MAX_DELAY_MS: u64 = 30_000;
const ANIMATION_SCAN_AHEAD_PAGES: usize = 5;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 3.0;
const ZOOM_STEP: f32 = 1.1;
const ZOOM_RELOAD_DELAY: Duration = Duration::from_secs(1);
const FULLSCREEN_TOP_CHROME_REVEAL_HEIGHT: f32 = 20.0;
const FULLSCREEN_TITLE_BAR_HEIGHT: f32 = 28.0;

pub struct MangaReader {
    zip_path: Option<PathBuf>,
    image_files: Vec<String>,
    current_index: usize,
    textures: [Option<egui::TextureHandle>; 2],
    buffer_next: [Option<egui::TextureHandle>; 2],
    buffer_prev: [Option<egui::TextureHandle>; 2],
    top_down_textures: Vec<Option<egui::TextureHandle>>,
    top_down_scroll_offset: f32,
    last_buffered_index: Option<usize>,
    all_zips_in_folder: Vec<PathBuf>,
    error_msg: Option<(String, Instant)>,
    is_fullscreen: bool,
    can_scroll: bool,
    dialog_rx: Receiver<Option<PathBuf>>,
    dialog_tx: Sender<Option<PathBuf>>,
    page_indicator_time: Option<Instant>,
    last_dialog_time: std::time::Instant,
    is_dialog_open: bool,
    zip_name_display: Option<(String, Instant)>,
    is_shifted: bool,
    config: AppSettings,
    binding_action: Option<BindingTarget>,
    texture_cache: std::collections::HashMap<String, egui::TextureHandle>,
    image_dimensions: std::collections::HashMap<String, [u32; 2]>,
    active_animations: [Option<AnimatedPage>; 2],
    active_animation_keys: [Option<String>; 2],
    animated_image_byte_cache: std::collections::HashMap<String, Vec<u8>>,
    animated_image_first_frame_cache: std::collections::HashMap<String, AnimatedFrame>,
    animation_scan_anchor: Option<usize>,
    animation_scan_generation: u64,
    animation_scan_epoch: Arc<AtomicU64>,
    animation_scan_rx: Option<Receiver<AnimationScanEvent>>,
    animation_scan_status: std::collections::HashMap<String, AnimationScanStatus>,
    pdfium: Option<&'static Pdfium>,
    pdf_document: Option<PdfDocument<'static>>,
    pdf_document_path: Option<PathBuf>,
    initial_path: Option<PathBuf>,
    source_mode: SourceMode,
    last_image_switch_time: Instant,
    auto_scroll_enabled: bool,
    auto_scroll_next_tick: Option<Instant>,
    zoom_factor: f32,
    pan_offset: egui::Vec2,
    last_page_view_rect: Option<egui::Rect>,
    last_zoom_change_at: Option<Instant>,
    zoom_reload_pending: bool,
    is_scrubbing: bool,
    mouse_press_started: [Option<Instant>; 5],
    mouse_press_origin: [Option<egui::Pos2>; 5],
    mouse_long_triggered: [bool; 5],
    mouse_drag_suppressed: [bool; 5],
    pending_mouse_click: [Option<(Instant, MangaAction)>; 5],
    gamepad: Option<Gilrs>,
    gamepad_repeat_deadlines: [Option<Instant>; 16],
    selected_control_profile: ControlProfile,
    settings_toggle_last_visible_at: Instant,
    fullscreen_top_chrome_visible: bool,
}

impl MangaReader {
    pub fn new(_cc: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        font::setup_custom_fonts(&_cc.egui_ctx);
        let config = settings_store::load();
        set_language(config.language);

        Self::with_settings(config, initial_path)
    }

    fn with_settings(config: AppSettings, initial_path: Option<PathBuf>) -> Self {
        let (tx, rx) = channel();
        Self {
            initial_path,
            zip_path: None,
            image_files: Vec::new(),
            current_index: 0,
            textures: [None, None],
            buffer_next: [None, None],
            buffer_prev: [None, None],
            top_down_textures: Vec::new(),
            top_down_scroll_offset: 0.0,
            last_buffered_index: None,
            all_zips_in_folder: Vec::new(),
            error_msg: None,
            dialog_rx: rx,
            dialog_tx: tx,
            is_fullscreen: false,
            can_scroll: true,
            page_indicator_time: None,
            last_dialog_time: Instant::now(),
            is_dialog_open: false,
            zip_name_display: None,
            is_shifted: false,
            config, // Store the loaded config here
            binding_action: None,
            texture_cache: Default::default(),
            image_dimensions: Default::default(),
            active_animations: std::array::from_fn(|_| None),
            active_animation_keys: std::array::from_fn(|_| None),
            animated_image_byte_cache: Default::default(),
            animated_image_first_frame_cache: Default::default(),
            animation_scan_anchor: None,
            animation_scan_generation: 0,
            animation_scan_epoch: Arc::new(AtomicU64::new(0)),
            animation_scan_rx: None,
            animation_scan_status: Default::default(),
            pdfium: None,
            pdf_document: None,
            pdf_document_path: None,
            source_mode: SourceMode::Zip,
            last_image_switch_time: Instant::now(),
            auto_scroll_enabled: false,
            auto_scroll_next_tick: None,
            zoom_factor: 1.0,
            pan_offset: egui::Vec2::ZERO,
            last_page_view_rect: None,
            last_zoom_change_at: None,
            zoom_reload_pending: false,
            is_scrubbing: false,
            mouse_press_started: [None; 5],
            mouse_press_origin: [None; 5],
            mouse_long_triggered: [false; 5],
            mouse_drag_suppressed: [false; 5],
            pending_mouse_click: [None; 5],
            gamepad: Gilrs::new().ok(),
            gamepad_repeat_deadlines: [None; 16],
            selected_control_profile: ControlProfile::Default,
            settings_toggle_last_visible_at: Instant::now(),
            fullscreen_top_chrome_visible: false,
        }
    }

    fn save_settings(&self) {
        settings_store::save(&self.config);
    }

    fn show_fading_error(&mut self, msg: &str) {
        self.error_msg = Some((msg.to_string(), Instant::now()));
    }
}
impl eframe::App for MangaReader {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        set_language(self.config.language);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(tr("app.title").to_owned()));
        self.sync_animation_scan(ctx);
        self.update_active_animations(ctx);

        // load file if it is dropped on screen
        let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(df) = dropped_files.first() {
            if let Some(path) = &df.path {
                self.load_source(path.clone(), ctx);
            } else if let Some(_bytes) = &df.bytes {
                self.show_fading_error(tr("error.dropped_bytes_only"));
            }
        }

        let mut action_to_run = self.collect_input_action(ctx);

        // Load file if passed as program parameter
        if let Some(p) = self.initial_path.as_ref() {
            self.load_source(p.clone(), ctx);
            self.initial_path = None;
        }

        // File Dialog Result
        if let Ok(result) = self.dialog_rx.try_recv() {
            self.is_dialog_open = false;
            if let Some(path) = result {
                self.load_source(path, ctx);
            }
        }
        self.reload_zoom_textures_if_settled(ctx);

        self.render_settings_panel(ctx);
        self.render_settings_toggle(ctx);
        self.render_top_toolbar(ctx);
        self.tick_auto_scroll(ctx, action_to_run);

        self.render_page_view(ctx, &mut action_to_run);

        if action_to_run != MangaAction::None {
            self.execute_action(action_to_run, ctx);
        }

        // Keep preloading buffers
        self.update_buffers(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppSettings, LastPageAction, PageViewOptions, ResizeMethod};
    use crate::engine::layout::automatic_pair_count;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestPages(PathBuf);

    impl TestPages {
        fn new(sizes: &[[u32; 2]]) -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "manga-layout-test-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            for (index, [width, height]) in sizes.iter().copied().enumerate() {
                image::RgbaImage::new(width, height)
                    .save(path.join(format!("{index:03}.png")))
                    .unwrap();
            }
            Self(path)
        }

        fn reader(&self, config: AppSettings, ctx: &egui::Context) -> MangaReader {
            let mut reader = MangaReader::with_settings(config, None);
            reader.load_source(self.0.clone(), ctx);
            reader
        }
    }

    impl Drop for TestPages {
        fn drop(&mut self) {
            for file in fs::read_dir(&self.0).unwrap().flatten() {
                fs::remove_file(file.path()).unwrap();
            }
            fs::remove_dir(&self.0).unwrap();
        }
    }

    fn assert_view(reader: &MangaReader, index: usize, mode: PageViewOptions, count: usize) {
        assert_eq!(reader.current_index, index);
        assert_eq!(reader.config.page_view_options, mode);
        assert_eq!(reader.textures.iter().flatten().count(), count);
    }

    #[test]
    fn automatic_layout_navigates_mixed_pages_in_both_directions() {
        let pages = TestPages::new(&[
            [10, 20],
            [10, 20],
            [10, 20],
            [30, 10],
            [10, 20],
            [30, 10],
            [10, 20],
            [10, 20],
        ]);
        let ctx = egui::Context::default();
        for direction in [PageViewOptions::DoubleRL, PageViewOptions::DoubleLR] {
            for caching in [false, true] {
                for preloading in [false, true] {
                    let config = AppSettings {
                        auto_page_layout_rl: direction == PageViewOptions::DoubleRL,
                        auto_page_layout_lr: direction == PageViewOptions::DoubleLR,
                        enable_single_file_caching: caching,
                        enable_animated_images: false,
                        resize_method: ResizeMethod::None,
                        ..AppSettings::default()
                    };
                    let mut reader = pages.reader(config, &ctx);
                    let expected = [
                        (0, direction, 2),
                        (2, direction, 1),
                        (3, PageViewOptions::Single, 1),
                        (4, direction, 1),
                        (5, PageViewOptions::Single, 1),
                        (6, direction, 2),
                    ];
                    assert_view(&reader, 0, direction, 2);
                    for &(index, mode, count) in &expected[1..] {
                        if preloading {
                            reader.update_buffers(&ctx);
                        }
                        reader.next_page(&ctx, false);
                        assert_view(&reader, index, mode, count);
                    }
                    for &(index, mode, count) in expected[..expected.len() - 1].iter().rev() {
                        if preloading {
                            reader.update_buffers(&ctx);
                        }
                        reader.prev_page(&ctx, false);
                        assert_view(&reader, index, mode, count);
                    }
                }
            }
        }
    }

    #[test]
    fn disabled_layout_preserves_pair_loading_and_fixed_navigation() {
        let pages = TestPages::new(&[[10, 20], [30, 10], [10, 20], [30, 10]]);
        let ctx = egui::Context::default();
        for mode in [
            PageViewOptions::Single,
            PageViewOptions::DoubleRL,
            PageViewOptions::DoubleLR,
        ] {
            let mut reader = pages.reader(
                AppSettings {
                    page_view_options: mode,
                    enable_animated_images: false,
                    resize_method: ResizeMethod::None,
                    ..AppSettings::default()
                },
                &ctx,
            );
            // Single mode deliberately retains the legacy two-image preload.
            assert_view(&reader, 0, mode, 2);
            reader.update_buffers(&ctx);
            let step = if mode == PageViewOptions::Single {
                1
            } else {
                2
            };
            reader.next_page(&ctx, false);
            assert_view(&reader, step, mode, 2);
            reader.update_buffers(&ctx);
            reader.prev_page(&ctx, false);
            assert_view(&reader, 0, mode, 2);
        }
    }

    #[test]
    fn automatic_layout_handles_cover_last_page_and_forced_steps() {
        let pages = TestPages::new(&[[10, 20], [10, 20], [10, 20], [30, 10]]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                auto_page_layout_rl: true,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                last_page_action: LastPageAction::Nothing,
                ..AppSettings::default()
            },
            &ctx,
        );
        reader.is_shifted = true;
        reader.load_current_pair(&ctx);
        assert_view(&reader, 0, PageViewOptions::DoubleRL, 1);
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, false);
        assert_view(&reader, 1, PageViewOptions::DoubleRL, 2);
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, true);
        assert_view(&reader, 2, PageViewOptions::DoubleRL, 1);
        reader.go_to_last_page(&ctx);
        assert_view(&reader, 3, PageViewOptions::Single, 1);
        reader.prev_page(&ctx, false);
        assert_view(&reader, 1, PageViewOptions::DoubleRL, 2);
        reader.prev_page(&ctx, false);
        assert_view(&reader, 0, PageViewOptions::DoubleRL, 1);
        reader.prev_page(&ctx, false);
        assert_view(&reader, 0, PageViewOptions::DoubleRL, 1);
    }

    #[test]
    fn settings_missing_fields_fall_back_to_defaults() {
        let mut json = serde_json::to_value(AppSettings::default()).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("last_page_action");
        object.remove("show_top_bar");
        let config: AppSettings = serde_json::from_value(json).unwrap();
        assert_eq!(config.last_page_action, LastPageAction::GotoNextFile);
        assert!(config.show_top_bar);
    }

    #[test]
    fn bundled_settings_json_parses() {
        let data = include_str!("../../settings.json");
        serde_json::from_str::<AppSettings>(data).unwrap();
    }

    #[test]
    fn existing_settings_default_automatic_layout_to_disabled() {
        let mut json = serde_json::to_value(AppSettings::default()).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("auto_page_layout_rl");
        object.remove("auto_page_layout_lr");
        let config: AppSettings = serde_json::from_value(json).unwrap();
        assert!(!config.auto_page_layout_rl && !config.auto_page_layout_lr);
        assert_eq!(
            automatic_pair_count(Some([10, 10]), Some([10, 20]), false),
            1
        );
        assert_eq!(automatic_pair_count(Some([10, 20]), None, false), 1);
    }

    #[test]
    fn disabling_automatic_layout_restores_the_suppressed_second_page() {
        let pages = TestPages::new(&[[10, 20], [30, 10], [10, 20]]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                auto_page_layout_rl: true,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        assert_view(&reader, 0, PageViewOptions::DoubleRL, 1);
        reader.update_buffers(&ctx);
        reader.config.auto_page_layout_rl = false;
        reader.reset_buffer();
        reader.texture_cache.clear();
        reader.load_current_pair(&ctx);
        assert_view(&reader, 0, PageViewOptions::DoubleRL, 2);
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, false);
        assert_view(&reader, 2, PageViewOptions::DoubleRL, 1);

        reader.config.auto_page_layout_rl = true;
        reader.config.page_view_options = PageViewOptions::TopDown;
        let pair = reader.load_pair(0, &ctx);
        assert!(pair.iter().all(Option::is_some));
        assert_eq!(reader.automatic_page_direction(), None);
    }
}
