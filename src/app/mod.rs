// このファイルは RustMangaReader（Lycoris52、Apache License 2.0）のコードを元に改変したものです。

mod animation;
mod input;
mod loader;
mod navigation;
mod overlay;
mod pageview;
mod settings_panel;
mod thumbnails;
mod toolbar;
mod view;

use crate::config::{AppSettings, MangaAction, SourceMode};
use crate::engine::history::History;
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
/// スクロール1回で動かす量（表示領域の大きさに対する割合）
const SCROLL_STEP_RATIO: f32 = 0.8;
/// 固定ページ進む／戻るで動くページ数（ミーヤのマンガ用設定と同じ）
const JUMP_PAGE_COUNT: usize = 20;
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
    /// 押したまま他のボタンやホイールと組み合わせて使ったボタン。離したときにクリック扱いしない
    mouse_chord_used: [bool; 5],
    /// reset_pan のあと、ページを読み始めの位置に合わせるのを待っている
    pan_to_start_pending: bool,
    loupe_enabled: bool,
    /// このフレームで描いたページ。ルーペはこれを拡大して描く
    drawn_pages: Vec<view::DrawnPage>,
    pending_mouse_click: [Option<(Instant, MangaAction)>; 5],
    gamepad: Option<Gilrs>,
    gamepad_repeat_deadlines: [Option<Instant>; 16],
    selected_control_profile: ControlProfile,
    settings_toggle_last_visible_at: Instant,
    fullscreen_top_chrome_visible: bool,
    /// 全画面で、下に置いたツールバーを出しているか
    fullscreen_bottom_toolbar_visible: bool,
    history: History,
    /// 履歴の保存先。None なら保存しない（テスト用）
    history_path: Option<PathBuf>,
    /// サブフォルダを読むかを聞いているあいだの、開こうとしている冊
    subfolder_question: Option<loader::SubfolderQuestion>,
    /// 自動見開きがオンのまま利用者がページ表示を選んだ。次にページをめくるまで、
    /// 自動見開きより選んだ表示を優先する（ミーヤと同じ）
    manual_page_view: bool,
    /// 左右に分けて見せている横長のページで、後に読む半分を見せているページ
    split_second_half_of: Option<usize>,
    /// 開いているサムネイル一覧
    thumbnails: Option<thumbnails::ThumbnailView>,
    thumbnail_cache: thumbnails::ThumbnailCache,
    /// 次に表示し直すとき、見開きの組を合わせ直さない（サムネイルで選んだページから見開きにする）
    keep_spread_start: bool,
}

impl MangaReader {
    pub fn new(_cc: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        font::setup_custom_fonts(&_cc.egui_ctx);
        let config = settings_store::load();
        set_language(config.language);

        let mut reader = Self::with_settings(config, initial_path);
        let history_path = settings_store::history_path();
        reader.history = History::load(&history_path);
        reader.history_path = Some(history_path);
        reader
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
            mouse_chord_used: [false; 5],
            pan_to_start_pending: false,
            loupe_enabled: false,
            drawn_pages: Vec::new(),
            pending_mouse_click: [None; 5],
            gamepad: Gilrs::new().ok(),
            gamepad_repeat_deadlines: [None; 16],
            selected_control_profile: ControlProfile::Default,
            settings_toggle_last_visible_at: Instant::now(),
            fullscreen_top_chrome_visible: false,
            fullscreen_bottom_toolbar_visible: false,
            history: History::default(),
            history_path: None,
            subfolder_question: None,
            manual_page_view: false,
            split_second_half_of: None,
            thumbnails: None,
            thumbnail_cache: Default::default(),
            keep_spread_start: false,
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
                self.open_source(path.clone(), ctx);
            } else if let Some(_bytes) = &df.bytes {
                self.show_fading_error(tr("error.dropped_bytes_only"));
            }
        }

        let mut action_to_run = self.collect_input_action(ctx);

        // Load file if passed as program parameter
        if let Some(p) = self.initial_path.as_ref() {
            self.open_source(p.clone(), ctx);
            self.initial_path = None;
        }

        // File Dialog Result
        if let Ok(result) = self.dialog_rx.try_recv() {
            self.is_dialog_open = false;
            if let Some(path) = result {
                self.open_source(path, ctx);
            }
        }
        self.reload_zoom_textures_if_settled(ctx);

        self.render_settings_panel(ctx);
        self.render_settings_toggle(ctx);
        self.render_toolbar(ctx);
        self.tick_auto_scroll(ctx, action_to_run);

        if self.thumbnails.is_some() {
            // 一覧を開いているあいだは、一覧の切り替え以外の操作を受け付けない
            if !matches!(
                action_to_run,
                MangaAction::ThumbnailsFitAll
                    | MangaAction::ThumbnailsLarge
                    | MangaAction::CoverThumbnails
                    | MangaAction::UpperFolder
            ) {
                action_to_run = MangaAction::None;
            }
            self.render_thumbnails(ctx);
        } else {
            self.render_page_view(ctx, &mut action_to_run);
        }
        self.render_subfolder_question(ctx);
        // ダイアログに答えるまでは、ページの操作を受け付けない
        if self.subfolder_question.is_some() {
            action_to_run = MangaAction::None;
        }

        if action_to_run != MangaAction::None {
            self.execute_action(action_to_run, ctx);
        }

        // Keep preloading buffers
        self.update_buffers(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.remember_current_book();
        self.save_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppSettings, LastPageAction, PageViewOptions, ResizeMethod, Shortcut};
    use crate::engine::history;
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
            reader.load_source(self.0.clone(), false, ctx);
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
                    // 戻るときは直前の2枚が縦長なら組む（ミーヤと同じ）ので、行きと組み方が変わる
                    let backward = [
                        (5, PageViewOptions::Single, 1),
                        (4, direction, 1),
                        (3, PageViewOptions::Single, 1),
                        (1, direction, 2),
                        (0, direction, 2),
                    ];
                    for &(index, mode, count) in &backward {
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
    fn automatic_layout_goes_back_like_mangameeya() {
        // 縦縦縦横縦横縦縦横横縦縦縦 と並ぶ本
        let (p, l) = ([10, 20], [20, 10]);
        let pages = TestPages::new(&[p, p, p, l, p, l, p, p, l, l, p, p, p]);
        let ctx = egui::Context::default();
        let config = || AppSettings {
            auto_page_layout_rl: true,
            enable_animated_images: false,
            resize_method: ResizeMethod::None,
            last_page_action: LastPageAction::Nothing,
            image_delay: 0,
            ..AppSettings::default()
        };
        // 表示中の組を 1 始まりのページ番号で返す
        let view = |reader: &MangaReader| {
            let first = reader.current_index + 1;
            let count = reader.textures.iter().flatten().count();
            if reader.config.page_view_options == PageViewOptions::Single || count == 1 {
                vec![first]
            } else {
                vec![first, first + 1]
            }
        };
        let mut reader = pages.reader(config(), &ctx);
        let mut forward = vec![view(&reader)];
        for _ in 0..9 {
            reader.update_buffers(&ctx);
            reader.next_page(&ctx, false);
            forward.push(view(&reader));
        }
        let expected: [&[usize]; 10] = [
            &[1, 2],
            &[3],
            &[4],
            &[5],
            &[6],
            &[7, 8],
            &[9],
            &[10],
            &[11, 12],
            &[13],
        ];
        assert_eq!(forward, expected);
        let mut backward = Vec::new();
        for _ in 0..9 {
            reader.update_buffers(&ctx);
            reader.prev_page(&ctx, false);
            backward.push(view(&reader));
        }
        let expected: [&[usize]; 9] = [
            &[11, 12],
            &[10],
            &[9],
            &[7, 8],
            &[6],
            &[5],
            &[4],
            &[2, 3],
            &[1, 2],
        ];
        assert_eq!(backward, expected);

        // portrait.zip: 1つ進んでずれた組から戻ると、直前の2枚で組む
        let pages = TestPages::new(&[p; 10]);
        let mut reader = pages.reader(config(), &ctx);
        reader.next_page(&ctx, true);
        assert_eq!(view(&reader), [2, 3]);
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, false);
        assert_eq!(view(&reader), [4, 5]);
        reader.update_buffers(&ctx);
        reader.prev_page(&ctx, false);
        assert_eq!(view(&reader), [2, 3]);
        reader.update_buffers(&ctx);
        reader.prev_page(&ctx, false);
        assert_eq!(view(&reader), [1, 2]);
    }

    #[test]
    fn last_lone_page_is_paired_with_a_blank_like_mangameeya() {
        // 白紙は後に読む側（右開きなら左）
        let (p, l) = ([10, 20], [20, 10]);
        let odd = TestPages::new(&[p; 9]);
        let mixed = TestPages::new(&[p, p, p, l, p, l, p, p, l, l, p, p, p]);
        let ctx = egui::Context::default();
        let config = |auto: bool, cover_blank_page: bool| AppSettings {
            auto_page_layout_rl: auto,
            cover_blank_page,
            enable_animated_images: false,
            resize_method: ResizeMethod::None,
            last_page_action: LastPageAction::Nothing,
            image_delay: 0,
            ..AppSettings::default()
        };
        // 右開きでは textures[0] が右、textures[1] が左
        let trailing_blank = |reader: &MangaReader| {
            reader.textures[0].is_some()
                && !reader.is_cover_blank(&reader.textures[0])
                && reader.is_cover_blank(&reader.textures[1])
        };

        // 2ページ表示: 最後の (9) は白紙と並ぶ。戻ると白紙は消える
        let mut reader = odd.reader(config(false, false), &ctx);
        for _ in 0..4 {
            reader.update_buffers(&ctx);
            reader.next_page(&ctx, false);
        }
        assert_eq!(reader.current_index, 8);
        assert!(trailing_blank(&reader));
        reader.update_buffers(&ctx);
        reader.prev_page(&ctx, false);
        assert_eq!(reader.current_index, 6);
        assert!(!reader.is_cover_blank(&reader.textures[1]));
        reader.go_to_last_page(&ctx);
        assert!(trailing_blank(&reader));

        // 自動見開きでは、最後の (9) は単ページ
        let mut reader = odd.reader(config(true, false), &ctx);
        reader.go_to_last_page(&ctx);
        assert_eq!(reader.current_index, 8);
        assert!(!reader.is_cover_blank(&reader.textures[1]));

        // 1枚目を単独で中央に見せる奇数ページ開始では、最後の1枚も白紙と並べない
        let pages = TestPages::new(&[p; 10]);
        let mut reader = pages.reader(config(false, false), &ctx);
        reader.change_shifted_mode(&ctx);
        reader.go_to_last_page(&ctx);
        assert_eq!(reader.current_index, 9);
        assert!(!reader.is_cover_blank(&reader.textures[1]));

        // 先頭ページ挿入と自動見開き: (白紙,1) … (11,12) (13,白紙)。途中の (5) は単ページのまま
        let mut reader = mixed.reader(config(true, true), &ctx);
        reader.change_shifted_mode(&ctx);
        assert!(reader.is_cover_blank(&reader.textures[0]));
        let mut seen_trailing = Vec::new();
        for _ in 0..9 {
            reader.update_buffers(&ctx);
            reader.next_page(&ctx, false);
            if reader.is_cover_blank(&reader.textures[1]) {
                seen_trailing.push(reader.current_index + 1);
            }
        }
        assert_eq!(reader.current_index, 12);
        assert_eq!(seen_trailing, [13]);
        assert!(trailing_blank(&reader));
        // 先頭まで戻ると、先頭の白紙が正しく並ぶ
        for _ in 0..9 {
            reader.update_buffers(&ctx);
            reader.prev_page(&ctx, false);
        }
        assert_eq!(reader.current_index, 0);
        assert!(reader.is_cover_blank(&reader.textures[0]));
        assert!(!reader.is_cover_blank(&reader.textures[1]));

        // 2ページだけの本で、先頭と最後の白紙を行き来しても取り違えない
        let pages = TestPages::new(&[p; 2]);
        let mut reader = pages.reader(config(false, true), &ctx);
        reader.change_shifted_mode(&ctx);
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, false);
        assert!(trailing_blank(&reader));
        reader.update_buffers(&ctx);
        reader.prev_page(&ctx, false);
        assert!(reader.is_cover_blank(&reader.textures[0]));
        assert!(!reader.is_cover_blank(&reader.textures[1]));
    }

    #[test]
    fn wide_pages_are_split_into_halves_like_mangameeya() {
        // 縦 横 縦 縦 横 横 縦 と並ぶ本
        let (p, l) = ([10, 20], [20, 10]);
        let pages = TestPages::new(&[p, l, p, p, l, l, p]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                split_landscape_pages: true,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                last_page_action: LastPageAction::Nothing,
                image_delay: 0,
                ..AppSettings::default()
            },
            &ctx,
        );
        // 見えている部分を「2右」のように書く。ページ番号は分けても増えない
        let view = |reader: &MangaReader| {
            let page = reader.current_index + 1;
            match reader.split_half_shows_right() {
                Some(true) => format!("{page}右"),
                Some(false) => format!("{page}左"),
                None => page.to_string(),
            }
        };
        let expected = ["1", "2右", "2左", "3", "4", "5右", "5左", "6右", "6左", "7"];
        let mut forward = vec![view(&reader)];
        for _ in 1..expected.len() {
            reader.update_buffers(&ctx);
            reader.next_page(&ctx, false);
            forward.push(view(&reader));
        }
        assert_eq!(forward, expected);
        // 戻るときは逆の順（横長に戻ったら後に読む半分から）
        let mut backward = vec![view(&reader)];
        for _ in 1..expected.len() {
            reader.update_buffers(&ctx);
            reader.prev_page(&ctx, false);
            backward.push(view(&reader));
        }
        backward.reverse();
        assert_eq!(backward, expected);

        // 見せる半分だけが画面の中央に来る。右半分なら画像全体は左にずれる
        reader.go_to_page(1, &ctx);
        let center = egui::pos2(500.0, 400.0);
        let (full, visible) = reader
            .single_page_rects(center, egui::vec2(1000.0, 800.0))
            .unwrap();
        assert_eq!(visible.center(), center);
        assert!((full.width() - visible.width() * 2.0).abs() < 0.01);
        assert!((full.max.x - visible.max.x).abs() < 0.01);

        // 左開きなら左半分が先
        reader.config.left_to_right = true;
        assert_eq!(view(&reader), "2左");
        reader.next_page(&ctx, false);
        assert_eq!(view(&reader), "2右");
        reader.config.left_to_right = false;

        // 見開きを選ぶと切れる。オンにすると1ページ表示に戻る
        reader.config.choose_page_view(PageViewOptions::DoubleRL);
        assert!(!reader.config.split_landscape_pages);
        reader.toggle_split_landscape_pages(&ctx);
        assert!(reader.config.split_landscape_pages);
        assert_eq!(reader.config.page_view_options, PageViewOptions::Single);
        // 自動見開きをオンにしても切れる
        reader.toggle_auto_page_layout(&ctx);
        assert!(!reader.config.split_landscape_pages);
        // 自動見開きのときにオンにすると、自動見開きが切れて1ページ表示になる
        reader.toggle_split_landscape_pages(&ctx);
        assert!(reader.config.split_landscape_pages);
        assert!(!reader.config.auto_page_layout_rl && !reader.config.auto_page_layout_lr);
        assert_eq!(reader.config.page_view_options, PageViewOptions::Single);
        assert!(reader.config.reads_right_to_left());
    }

    #[test]
    fn thumbnails_load_in_the_background_and_open_the_chosen_page() {
        use crate::engine::thumbnail::ThumbnailMode;
        let pages = TestPages::new(&[[10, 20]; 10]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        reader.go_to_page(2, &ctx);
        reader.open_thumbnails(ThumbnailMode::FitAll);
        assert!(reader.thumbnails.is_some());
        // 別スレッドが作り終えるまで受け取る
        for _ in 0..500 {
            let _ = ctx.run(Default::default(), |ctx| reader.render_thumbnails(ctx));
            if reader.thumbnail_cache.textures.iter().all(Option::is_some) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(reader.thumbnail_cache.textures.iter().all(Option::is_some));

        // 同じ種類をもう一度開くと閉じる。開き直しても作り直さない
        reader.open_thumbnails(ThumbnailMode::FitAll);
        assert!(reader.thumbnails.is_none());
        reader.open_thumbnails(ThumbnailMode::Large);
        assert!(reader.thumbnails.as_ref().unwrap().rx_is_idle());

        // 見開きでも、選んだページから始まる組にする（ミーヤと同じ。6 なら (6,7)）
        reader.thumbnails = None;
        reader.show_page_from_thumbnail(5, &ctx);
        assert_view(&reader, 5, PageViewOptions::DoubleRL, 2);
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, false);
        assert_view(&reader, 7, PageViewOptions::DoubleRL, 2);

        // 冊が変わると、前の冊のサムネイルは捨てる
        let other = TestPages::new(&[[10, 20]; 3]);
        reader.open_thumbnails(ThumbnailMode::FitAll);
        reader.load_source(other.0.clone(), false, &ctx);
        assert!(reader.thumbnails.is_none());
        assert!(reader.thumbnail_cache.textures.is_empty());
    }

    #[test]
    fn cover_thumbnails_list_the_books_beside_the_current_one() {
        use std::io::Write;
        let shelf = TestPages::new(&[]);
        let png = |width, height| {
            let mut bytes = Vec::new();
            image::RgbaImage::new(width, height)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            bytes
        };
        let write_zip = |path: PathBuf, pages: usize| {
            let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
            for page in 0..pages {
                zip.start_file(
                    format!("{page:02}.png"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.write_all(&png(10, 20)).unwrap();
            }
            zip.finish().unwrap();
        };
        // 書庫 2 冊、画像のフォルダ、冊の入ったフォルダが並ぶフォルダ
        let (a, b) = (shelf.0.join("01 A.zip"), shelf.0.join("02 B.zip"));
        write_zip(a.clone(), 3);
        write_zip(b.clone(), 3);
        let c = shelf.0.join("03 C");
        fs::create_dir(&c).unwrap();
        fs::write(c.join("01.png"), png(10, 20)).unwrap();
        let nested = shelf.0.join("04 shelf");
        fs::create_dir(&nested).unwrap();
        write_zip(nested.join("D.zip"), 2);

        let ctx = egui::Context::default();
        let mut reader = MangaReader::with_settings(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            None,
        );
        reader.open_source(b.clone(), &ctx);
        reader.go_to_page(1, &ctx);

        // フォルダが先。今の冊（B）が選ばれている
        reader.toggle_cover_thumbnails();
        let view = reader.thumbnails.as_ref().unwrap();
        assert_eq!(
            view.cover_paths(),
            [c.clone(), nested.clone(), a.clone(), b.clone()]
        );
        assert_eq!(view.selected(), 3);

        // 冊を選ぶとしおりから開く（ミーヤと同じ）
        reader.choose_thumbnail(2, &ctx);
        assert!(reader.thumbnails.is_none());
        assert_eq!(reader.zip_path, Some(a.clone()));
        reader.toggle_cover_thumbnails();
        reader.choose_thumbnail(3, &ctx);
        assert_eq!(reader.zip_path, Some(b.clone()));
        assert_eq!(reader.current_index, 1);

        // 冊の入ったフォルダを選ぶと、その中の冊の一覧に移る。上のフォルダへで戻る
        reader.toggle_cover_thumbnails();
        reader.choose_thumbnail(1, &ctx);
        let view = reader.thumbnails.as_ref().unwrap();
        assert_eq!(view.cover_paths(), [nested.join("D.zip")]);
        reader.go_to_upper_folder();
        let view = reader.thumbnails.as_ref().unwrap();
        assert_eq!(view.cover_paths().len(), 4);
        assert_eq!(view.selected(), 1);

        // 画像のフォルダは冊として開く。その表紙サムネイルは、入っているフォルダの冊を並べる
        reader.choose_thumbnail(0, &ctx);
        assert_eq!(reader.zip_path, Some(c.clone()));
        reader.toggle_cover_thumbnails();
        assert_eq!(reader.thumbnails.as_ref().unwrap().selected(), 0);
        // もう一度で閉じる
        reader.toggle_cover_thumbnails();
        assert!(reader.thumbnails.is_none());

        // 開いている冊からの上のフォルダへは、冊が入っているフォルダの一覧
        reader.go_to_upper_folder();
        assert_eq!(reader.thumbnails.as_ref().unwrap().cover_paths().len(), 4);

        for path in [a, b] {
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir_all(c).unwrap();
        fs::remove_dir_all(nested).unwrap();
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
    fn go_to_last_page_respects_single_and_shifted_layouts() {
        // (表示モード, 奇数ずらし, ページ数, 期待する最終表示の先頭)
        let cases = [
            (PageViewOptions::Single, false, 6, 5),
            (PageViewOptions::Single, false, 5, 4),
            (PageViewOptions::DoubleRL, false, 6, 4),
            (PageViewOptions::DoubleRL, false, 5, 4),
            (PageViewOptions::DoubleRL, true, 6, 5),
            (PageViewOptions::DoubleRL, true, 5, 3),
            (PageViewOptions::DoubleRL, true, 1, 0),
        ];
        let ctx = egui::Context::default();
        for (mode, shifted, len, expected) in cases {
            let pages = TestPages::new(&vec![[10, 20]; len]);
            let mut reader = pages.reader(
                AppSettings {
                    page_view_options: mode,
                    enable_animated_images: false,
                    resize_method: ResizeMethod::None,
                    ..AppSettings::default()
                },
                &ctx,
            );
            reader.is_shifted = shifted;
            reader.go_to_last_page(&ctx);
            assert_eq!(reader.current_index, expected, "{mode:?} {shifted} {len}");
        }
    }

    #[test]
    fn going_back_from_the_first_page_opens_the_previous_book_at_its_last_page() {
        // 隣り合う2冊を、他のフォルダが混ざらない専用の親フォルダに作る
        let parent = TestPages::new(&[]);
        for (book, count) in [("a", 5), ("b", 3)] {
            let dir = parent.0.join(book);
            fs::create_dir(&dir).unwrap();
            for index in 0..count {
                image::RgbaImage::new(10, 20)
                    .save(dir.join(format!("{index:03}.png")))
                    .unwrap();
            }
        }
        let ctx = egui::Context::default();
        for auto_layout in [false, true] {
            let mut reader = MangaReader::with_settings(
                AppSettings {
                    page_view_options: PageViewOptions::DoubleRL,
                    auto_page_layout_rl: auto_layout,
                    last_page_action: LastPageAction::GotoNextFile,
                    enable_animated_images: false,
                    resize_method: ResizeMethod::None,
                    ..AppSettings::default()
                },
                None,
            );
            reader.load_source(parent.0.join("b"), false, &ctx);
            reader.prev_page(&ctx, false);
            assert_eq!(reader.zip_path, Some(parent.0.join("a")));
            assert_eq!(reader.current_index, 4, "auto_layout={auto_layout}");

            // 明示的な「前のファイル」は従来どおり先頭から開く
            reader.load_source(parent.0.join("b"), false, &ctx);
            reader.prev_zip(&ctx, false);
            assert_eq!(reader.zip_path, Some(parent.0.join("a")));
            assert_eq!(reader.current_index, 0);
        }
        for book in ["a", "b"] {
            fs::remove_dir_all(parent.0.join(book)).unwrap();
        }
    }

    /// 1フレーム分のマウス入力を与え、画面全体を覆う領域で拾った操作を返す
    fn mouse_frame(
        reader: &mut MangaReader,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Option<MangaAction> {
        let pos = egui::pos2(100.0, 100.0);
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 400.0),
            )),
            ..Default::default()
        };
        input.events.push(egui::Event::PointerMoved(pos));
        input.events.extend(events);
        let mut action = None;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.allocate_rect(ui.max_rect(), egui::Sense::click());
                action = reader.collect_mouse_action(&response, ctx);
            });
        });
        action
    }

    fn button(button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: egui::pos2(100.0, 100.0),
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn wheel(delta_y: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta_y),
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn mouse_chords_fire_their_action_and_swallow_the_held_button_click() {
        use crate::config::MouseConfig;
        use egui::PointerButton::{Primary, Secondary};
        let ctx = egui::Context::default();
        let mut reader = MangaReader::with_settings(
            AppSettings {
                mouse: MouseConfig::mangameeya(),
                ..AppSettings::default()
            },
            None,
        );
        assert_eq!(mouse_frame(&mut reader, &ctx, vec![]), None);

        // 右を押したまま左クリック → 1ページ進む。右を離してもクリック（前に戻る）にならない
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Primary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Primary, false)]),
            Some(MangaAction::OneNextPage)
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, false)]),
            None
        );

        // 左を押したまま右クリック → 1ページ戻る。左を離してもクリックにならない
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Primary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, false)]),
            Some(MangaAction::OnePrevPage)
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Primary, false)]),
            None
        );

        // 右を押したままホイール → 拡大・縮小。右を離してもクリックにならない
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![wheel(5.0)]),
            Some(MangaAction::ZoomIn)
        );
        assert_eq!(mouse_frame(&mut reader, &ctx, vec![]), None);
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![wheel(-5.0)]),
            Some(MangaAction::ZoomOut)
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, false)]),
            None
        );

        // 単独のクリックとホイールは同時押しにならない（ホイールはスクロール兼次へ）
        assert_eq!(mouse_frame(&mut reader, &ctx, vec![]), None);
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![wheel(-5.0)]),
            Some(MangaAction::ScrollNextPage)
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, false)]),
            Some(MangaAction::PrevPage)
        );
    }

    #[test]
    fn unassigned_chords_keep_the_previous_behavior() {
        use egui::PointerButton::{Primary, Secondary};
        let ctx = egui::Context::default();
        // MangaReeda の既定では同時押しは未割り当てで、これまでと同じ動きになる
        let mut reader = MangaReader::with_settings(
            AppSettings {
                mouse: crate::config::MouseConfig {
                    button1_double_click: MangaAction::None,
                    ..Default::default()
                },
                ..AppSettings::default()
            },
            None,
        );
        assert_eq!(mouse_frame(&mut reader, &ctx, vec![]), None);
        // 右を押したまま左クリックすると左クリックだけが効く
        // （他のボタンを離すと、押したままのボタンは egui がクリックと見なさなくなる）
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Primary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Primary, false)]),
            Some(MangaAction::NextPage)
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, false)]),
            None
        );

        // 右を押したままのホイールはページ送りで、右を離すと右クリックになる
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, true)]),
            None
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![wheel(-5.0)]),
            Some(MangaAction::NextPage)
        );
        assert_eq!(
            mouse_frame(&mut reader, &ctx, vec![button(Secondary, false)]),
            Some(MangaAction::PrevPage)
        );
    }

    fn pan(reader: &MangaReader) -> (f32, f32) {
        (reader.pan_offset.x, reader.pan_offset.y)
    }

    #[test]
    fn scroll_or_page_reads_a_tall_page_top_to_bottom_then_turns() {
        use crate::config::ImageSizingMode;
        // 幅に合わせると 100x200 になり、100x100 の画面から縦に2画面分はみ出す
        let pages = TestPages::new(&[[10, 20], [10, 20], [10, 20]]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                image_sizing_mode: ImageSizingMode::FitWidth,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        reader.last_page_view_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(100.0, 100.0),
        ));
        // 開いたページは上端から表示する
        reader.apply_pending_pan_start();
        assert_eq!(pan(&reader), (0.0, 50.0));

        reader.scroll_next_page(&ctx);
        assert_eq!(pan(&reader), (0.0, -30.0));
        reader.scroll_next_page(&ctx);
        assert_eq!(pan(&reader), (0.0, -50.0));
        // 下端まで読んだら次のページを上端から
        reader.scroll_next_page(&ctx);
        assert_eq!(reader.current_index, 1);
        assert_eq!(pan(&reader), (0.0, 50.0));
        reader.apply_pending_pan_start();
        assert_eq!(pan(&reader), (0.0, 50.0));

        // 上端から戻ると前のページを下端から
        reader.scroll_prev_page(&ctx);
        assert_eq!(reader.current_index, 0);
        assert_eq!(pan(&reader), (0.0, -50.0));
        reader.apply_pending_pan_start();
        assert_eq!(pan(&reader), (0.0, -50.0));

        // 方向を指定したスクロールは端で止まり、ページは送らない
        reader.scroll_view(0.0, 1.0, &ctx);
        assert_eq!(pan(&reader), (0.0, -50.0));
        assert_eq!(reader.current_index, 0);
        reader.scroll_view(0.0, -1.0, &ctx);
        assert_eq!(pan(&reader), (0.0, 30.0));
        reader.scroll_view(-1.0, 0.0, &ctx);
        assert_eq!(pan(&reader), (0.0, 30.0));

        // 普通の「次へ」で開いたページも上端から
        reader.next_page(&ctx, false);
        reader.apply_pending_pan_start();
        assert_eq!(pan(&reader), (0.0, 50.0));
    }

    #[test]
    fn scroll_or_page_on_a_zoomed_spread_starts_at_the_top_right_when_reading_right_to_left() {
        let pages = TestPages::new(&[[10, 20], [10, 20], [10, 20], [10, 20]]);
        let ctx = egui::Context::default();
        for (mode, start_x) in [
            (PageViewOptions::DoubleRL, -50.0),
            (PageViewOptions::DoubleLR, 50.0),
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
            reader.last_page_view_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(200.0, 100.0),
            ));
            // 高さに合わせて3倍: 1ページ 150x300、見開きで 300x300
            reader.zoom_factor = 3.0;
            reader.apply_pending_pan_start();
            assert_eq!(pan(&reader), (start_x, 100.0), "{mode:?}");

            // 縦を読み終えたら、読む方向の列へ移って上端に戻る
            let mut steps = 0;
            while pan(&reader).1 > -100.0 {
                reader.scroll_next_page(&ctx);
                steps += 1;
                assert!(steps < 10);
            }
            reader.scroll_next_page(&ctx);
            assert_eq!(reader.current_index, 0);
            // 横は1回 160 動かすが、反対側の端（-start_x）で止まる
            assert_eq!(pan(&reader), (-start_x, 100.0));
        }
    }

    #[test]
    fn loupe_draws_the_page_under_the_pointer() {
        let pages = TestPages::new(&[[10, 20]]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        let frame = |reader: &mut MangaReader| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                events: vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0))],
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                let mut action = MangaAction::None;
                reader.render_page_view(ctx, &mut action);
            })
        };
        let page_texture = reader.textures[0].as_ref().unwrap().id();
        let loupe_meshes = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter(|clipped| {
                    clipped.shape.texture_id() == page_texture
                        && matches!(clipped.shape, egui::Shape::Mesh(_))
                })
                .count()
        };

        // ルーペを出していないときはページを1回描くだけ（ページの画像も Mesh として描かれる）
        let output = frame(&mut reader);
        assert_eq!(reader.drawn_pages.len(), 1);
        let without_loupe = loupe_meshes(&output);

        // ルーペを出すと、同じページの画像を使った拡大の図形が1つ加わる
        reader.loupe_enabled = true;
        let output = frame(&mut reader);
        assert_eq!(loupe_meshes(&output), without_loupe + 1);
    }

    #[test]
    fn choosing_a_page_view_is_not_undone_by_automatic_layout() {
        // 縦長のページなので、自動見開きがあると見開きに戻されていた
        let pages = TestPages::new(&[[10, 20]; 5]);
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
        assert_view(&reader, 0, PageViewOptions::DoubleRL, 2);
        // 設定画面で選んだときと同じ手順
        let choose = |reader: &mut MangaReader, mode| {
            reader.manual_page_view = reader.config.choose_page_view(mode);
            reader.reset_buffer();
            reader.texture_cache.clear();
            reader.load_current_pair(&ctx);
        };

        // 左から右を選ぶと、自動見開きも左から右になる
        choose(&mut reader, PageViewOptions::DoubleLR);
        assert_view(&reader, 0, PageViewOptions::DoubleLR, 2);
        assert!(reader.config.auto_page_layout_lr && !reader.config.auto_page_layout_rl);

        // 右から左に戻すと、自動見開きも右から左に戻る
        choose(&mut reader, PageViewOptions::DoubleRL);
        assert_view(&reader, 0, PageViewOptions::DoubleRL, 2);
        assert!(reader.config.auto_page_layout_rl && !reader.config.auto_page_layout_lr);

        // ミーヤと同じく、自動見開きのまま単ページを選ぶと、その場は単ページになり、
        // 自動見開きはオンのまま（単ページ表示でも先読みでテクスチャは2枚）
        choose(&mut reader, PageViewOptions::Single);
        assert_view(&reader, 0, PageViewOptions::Single, 2);
        assert!(reader.config.auto_page_layout_rl && !reader.config.auto_page_layout_lr);
        // 先読みがあっても、次のページは1枚だけ進み、そこからは自動見開きがまた見開きにする
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, false);
        assert_view(&reader, 1, PageViewOptions::DoubleRL, 2);
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, false);
        assert_view(&reader, 3, PageViewOptions::DoubleRL, 2);

        // 単ページにしたまま戻っても、自動見開きがまた表示を決める
        // （戻る先は縦長の続きの先頭から組んだ位置になる）
        choose(&mut reader, PageViewOptions::Single);
        reader.update_buffers(&ctx);
        reader.prev_page(&ctx, false);
        assert_eq!(reader.config.page_view_options, PageViewOptions::DoubleRL);
        assert_eq!(reader.textures.iter().flatten().count(), 2);
        assert!(!reader.manual_page_view);

        // 単ページを選んでから自動見開きをオフにすると、単ページのまま
        choose(&mut reader, PageViewOptions::Single);
        reader.toggle_auto_page_layout(&ctx);
        assert!(!reader.config.auto_page_layout_rl && !reader.config.auto_page_layout_lr);
        assert_eq!(reader.config.page_view_options, PageViewOptions::Single);

        // 自動見開きが切れているときに見開きを選んでも、自動見開きは入らない
        choose(&mut reader, PageViewOptions::DoubleLR);
        assert!(!reader.config.auto_page_layout_rl && !reader.config.auto_page_layout_lr);
        assert!(!reader.manual_page_view);
    }

    #[test]
    fn jumping_moves_twenty_pages_and_stops_at_the_ends() {
        let pages = TestPages::new(&[[10, 20]; 45]);
        let ctx = egui::Context::default();
        let config = || AppSettings {
            enable_animated_images: false,
            resize_method: ResizeMethod::None,
            last_page_action: LastPageAction::Nothing,
            ..AppSettings::default()
        };
        let mut reader = pages.reader(config(), &ctx);
        let rl = PageViewOptions::DoubleRL;
        // 見開きは (0,1), (2,3)… なので、最後の44ページ目は白紙と並ぶ
        for (index, count) in [(20, 2), (40, 2), (44, 2), (44, 2)] {
            reader.jump_pages(true, &ctx);
            assert_view(&reader, index, rl, count);
        }
        for index in [24, 4, 0, 0] {
            reader.jump_pages(false, &ctx);
            assert_view(&reader, index, rl, 2);
        }

        // 奇数ページ開始では、行き先の20ページ目を含む (19,20) の組から見せる
        reader.change_shifted_mode(&ctx);
        reader.jump_pages(true, &ctx);
        assert_view(&reader, 19, rl, 2);

        // 端で止まっているときは、リピート状態に従って反対の端へ戻る
        let mut reader = pages.reader(
            AppSettings {
                last_page_action: LastPageAction::ToFirstPage,
                ..config()
            },
            &ctx,
        );
        reader.go_to_last_page(&ctx);
        reader.jump_pages(true, &ctx);
        assert_view(&reader, 0, rl, 2);
    }

    #[test]
    fn jumping_with_automatic_layout_keeps_pairs_within_portrait_runs() {
        // 4ページ目が横長なので、その後の縦長は (5,6), (7,8)… と組む
        let mut sizes = [[10, 20]; 30];
        sizes[4] = [30, 10];
        let pages = TestPages::new(&sizes);
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
        let rl = PageViewOptions::DoubleRL;
        reader.jump_pages(true, &ctx);
        assert_view(&reader, 19, rl, 2);
        // 戻ると先頭の組 (0,1) に戻る
        reader.jump_pages(false, &ctx);
        assert_view(&reader, 0, rl, 2);
        // 最後の組は (27,28) の次の、単独の29ページ目
        reader.jump_pages(true, &ctx);
        reader.jump_pages(true, &ctx);
        assert_view(&reader, 29, rl, 1);
    }

    #[test]
    fn toggling_automatic_layout_keeps_the_reading_direction() {
        let pages = TestPages::new(&[[10, 20], [10, 20], [30, 10], [10, 20]]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        assert_view(&reader, 0, PageViewOptions::DoubleRL, 2);

        // オンにすると右から左の自動見開きになり、横長のページは1ページで見せる
        reader.toggle_auto_page_layout(&ctx);
        assert!(reader.config.auto_page_layout_rl && !reader.config.auto_page_layout_lr);
        reader.next_page(&ctx, false);
        assert_view(&reader, 2, PageViewOptions::Single, 1);

        // オフにすると、ミーヤと同じく必ず1ページ表示になる
        // （1ページ表示でも次のページを先読みするので、テクスチャは2枚）
        reader.toggle_auto_page_layout(&ctx);
        assert!(!reader.config.auto_page_layout_rl && !reader.config.auto_page_layout_lr);
        assert_view(&reader, 2, PageViewOptions::Single, 2);
        reader.go_to_first_page(&ctx);
        assert_view(&reader, 0, PageViewOptions::Single, 2);

        // 左から右の見開きからオンにすると、左から右の自動見開きになる
        let mut config = AppSettings {
            page_view_options: PageViewOptions::DoubleLR,
            ..AppSettings::default()
        };
        assert!(config.toggle_auto_page_layout());
        assert!(config.auto_page_layout_lr && !config.auto_page_layout_rl);
        // オフにすると1ページ表示になるが、向きは左から右のまま覚えていて、
        // もう一度オンにすると左から右の自動見開きになる。再起動しても同じ
        assert!(config.toggle_auto_page_layout());
        assert_eq!(config.page_view_options, PageViewOptions::Single);
        assert!(!config.reads_right_to_left());
        let mut config: AppSettings =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert!(config.toggle_auto_page_layout());
        assert_eq!(config.page_view_options, PageViewOptions::DoubleLR);
        assert!(config.auto_page_layout_lr && !config.auto_page_layout_rl);

        // 見開きを選んでいてもオフにすれば1ページ表示（設定画面のチェックボックスと同じ処理）
        config.set_auto_page_layout(None);
        config.choose_page_view(PageViewOptions::DoubleRL);
        config.set_auto_page_layout(Some(true));
        config.set_auto_page_layout(None);
        assert_eq!(config.page_view_options, PageViewOptions::Single);
        assert!(config.reads_right_to_left());

        // 左から右の見開きから1ページ表示を選んでも、向きは左から右のまま
        let mut config = AppSettings {
            page_view_options: PageViewOptions::DoubleLR,
            ..AppSettings::default()
        };
        config.choose_page_view(PageViewOptions::Single);
        assert!(!config.reads_right_to_left());

        // 縦スクロール表示では切り替えない
        let mut config = AppSettings {
            page_view_options: PageViewOptions::TopDown,
            ..AppSettings::default()
        };
        assert!(!config.toggle_auto_page_layout());
        assert_eq!(config.page_view_options, PageViewOptions::TopDown);
        assert!(!config.auto_page_layout_rl && !config.auto_page_layout_lr);
    }

    #[test]
    fn mangameeya_controls_also_give_the_top_down_view_the_loupe() {
        let mut config = AppSettings::default();
        config.apply_mangameeya_controls();
        let l = Some((egui::Key::L, false, false, false));
        let parts = |s: Option<Shortcut>| s.map(|s| (s.key, s.ctrl, s.alt, s.shift));
        assert_eq!(parts(config.keys.toggle_loupe), l);
        assert_eq!(parts(config.top_down_keys.toggle_loupe), l);
        assert_eq!(config.mouse.button3_click, MangaAction::ToggleLoupe);
        assert_eq!(
            config.top_down_mouse.button3_click,
            MangaAction::ToggleLoupe
        );
        // 縦スクロール表示では方向キーとホイールは縦スクロールのまま
        assert_eq!(
            parts(config.top_down_keys.slide_image_down),
            Some((egui::Key::ArrowDown, false, false, false))
        );
        assert_eq!(
            config.top_down_mouse.scroll_down,
            MangaAction::SlideImageDown
        );
        assert_eq!(
            config.top_down_mouse.button2_held_scroll_up,
            MangaAction::ZoomIn
        );
    }

    #[test]
    fn cover_blank_page_pairs_the_first_page_with_a_blank() {
        let pages = TestPages::new(&[[10, 20], [10, 20], [10, 20], [10, 20]]);
        let ctx = egui::Context::default();
        for (auto_layout, caching) in [(false, false), (false, true), (true, false)] {
            let mut reader = pages.reader(
                AppSettings {
                    page_view_options: PageViewOptions::DoubleRL,
                    auto_page_layout_rl: auto_layout,
                    enable_single_file_caching: caching,
                    cover_blank_page: true,
                    enable_animated_images: false,
                    resize_method: ResizeMethod::None,
                    ..AppSettings::default()
                },
                &ctx,
            );
            reader.change_shifted_mode(&ctx);
            let is_blank = |reader: &MangaReader| reader.is_cover_blank(&reader.textures[0]);
            // テクスチャの名前はページのファイル名
            let first_page = reader.image_files[0].clone();

            // 白紙が先に読む側（スロット0）、1枚目がその隣（スロット1）
            assert_view(&reader, 0, PageViewOptions::DoubleRL, 2);
            assert!(is_blank(&reader), "auto_layout={auto_layout}");
            assert_eq!(reader.textures[1].as_ref().unwrap().name(), first_page);
            // 見開きとして描く（1枚目を単独で中央に置かない）
            assert!(!reader.cover_shown_alone());

            // 2〜3枚目、戻ると白紙と1枚目
            reader.update_buffers(&ctx);
            reader.next_page(&ctx, false);
            assert_view(&reader, 1, PageViewOptions::DoubleRL, 2);
            assert!(!is_blank(&reader));
            reader.update_buffers(&ctx);
            reader.prev_page(&ctx, false);
            assert_view(&reader, 0, PageViewOptions::DoubleRL, 2);
            assert!(is_blank(&reader));

            // 設定を切ると1枚目が単独に戻る
            reader.config.cover_blank_page = false;
            reader.sync_cover_blank(&ctx);
            assert_view(&reader, 0, PageViewOptions::DoubleRL, 1);
            assert_eq!(reader.textures[0].as_ref().unwrap().name(), first_page);
            assert!(reader.cover_shown_alone());
        }
    }

    #[test]
    fn cover_blank_page_is_not_used_for_single_page_or_even_mode() {
        let pages = TestPages::new(&[[10, 20], [10, 20], [10, 20]]);
        let ctx = egui::Context::default();
        for (mode, shifted) in [
            (PageViewOptions::Single, true),
            (PageViewOptions::DoubleRL, false),
        ] {
            let mut reader = pages.reader(
                AppSettings {
                    page_view_options: mode,
                    cover_blank_page: true,
                    enable_animated_images: false,
                    resize_method: ResizeMethod::None,
                    ..AppSettings::default()
                },
                &ctx,
            );
            if shifted {
                reader.change_shifted_mode(&ctx);
            }
            assert!(!reader.is_cover_blank(&reader.textures[0]), "{mode:?}");
        }
    }

    fn shown_pages(reader: &MangaReader) -> Vec<String> {
        reader.textures.iter().flatten().map(|t| t.name()).collect()
    }

    #[test]
    fn automatic_layout_keeps_the_cover_next_to_its_blank() {
        // 先読みから白紙と1枚目の組が戻ってきたとき、自動見開きが「1枚だけ」と判断して
        // スロット1（1枚目）を消し、白紙だけが残っていた
        let pages = TestPages::new(&[[10, 20], [10, 20], [10, 20]]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                auto_page_layout_rl: true,
                cover_blank_page: true,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        reader.change_shifted_mode(&ctx);
        assert!(reader.is_cover_blank(&reader.textures[0]));
        reader.apply_automatic_page_layout();
        reader.sync_cover_blank(&ctx);
        assert!(reader.is_cover_blank(&reader.textures[0]));
        assert_eq!(shown_pages(&reader)[1], reader.image_files[0]);

        // 白紙だけが残っていても、1枚目を読み直して並べ直す
        reader.textures[1] = None;
        reader.sync_cover_blank(&ctx);
        assert_eq!(shown_pages(&reader)[1], reader.image_files[0]);
    }

    #[test]
    fn previous_page_after_an_unbuffered_turn_shows_the_right_pages() {
        // 先読みが間に合わずに直接読んだあと、前のページの先読みに古いページが残っていた
        let pages = TestPages::new(&[[10, 20]; 6]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                page_view_options: PageViewOptions::DoubleRL,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        reader.update_buffers(&ctx);
        reader.next_page(&ctx, false); // 先読みを使って 2〜3 へ
        reader.next_page(&ctx, false); // 先読みなしで 4〜5 へ
        assert_eq!(reader.current_index, 4);
        reader.prev_page(&ctx, false);
        assert_eq!(reader.current_index, 2);
        assert_eq!(shown_pages(&reader), reader.image_files[2..4]);
    }

    #[test]
    fn switching_to_a_spread_realigns_the_pairs() {
        // 奇数ページ開始の1ページ表示で偶数番目にいるとき見開きに切り替えると、組が1枚ずれていた
        let pages = TestPages::new(&[[10, 20]; 6]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        reader.change_shifted_mode(&ctx);
        reader.go_to_page(2, &ctx);
        reader.config.page_view_options = PageViewOptions::DoubleRL;
        reader.reset_buffer();
        reader.load_current_pair(&ctx);
        // 奇数ページ開始の組は (1,2), (3,4)…
        assert_eq!(reader.current_index, 1);
        assert_eq!(shown_pages(&reader), reader.image_files[1..3]);

        // 偶数ページ開始では (0,1), (2,3)…
        reader.change_shifted_mode(&ctx);
        reader.go_to_page(3, &ctx);
        assert_eq!(reader.current_index, 2);
    }

    #[test]
    fn match_height_gives_both_pages_of_a_spread_the_same_height() {
        use crate::config::ImageSizingMode;
        // 縦横比 0.5 と 0.7 の2枚
        let pages = TestPages::new(&[[50, 100], [70, 100]]);
        let ctx = egui::Context::default();
        let half = egui::vec2(100.0, 160.0);
        let sizes = |mode| {
            let reader = pages.reader(
                AppSettings {
                    page_view_options: PageViewOptions::DoubleRL,
                    image_sizing_mode: mode,
                    enable_animated_images: false,
                    resize_method: ResizeMethod::None,
                    ..AppSettings::default()
                },
                &ctx,
            );
            reader.spread_draw_sizes(half, 1.0)
        };
        // 幅と高さに合わせると、それぞれ半分に収めるので高さが違う
        let [first, second] = sizes(ImageSizingMode::FitBoth);
        assert_eq!(first, egui::vec2(80.0, 160.0));
        assert_ne!(first.y, second.y);
        // 縦サイズを揃えると、どちらも収まる高さにそろう
        let [first, second] = sizes(ImageSizingMode::MatchHeight);
        assert_eq!(first.y, second.y);
        assert!((second.x - 100.0).abs() < 1e-3);
        assert!(first.x <= half.x && first.y <= half.y);
    }

    #[test]
    fn shrink_only_keeps_small_pages_at_their_original_size() {
        use crate::config::ImageSizingMode;
        let pages = TestPages::new(&[[40, 60], [400, 600]]);
        let ctx = egui::Context::default();
        let mut reader = pages.reader(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                image_sizing_mode: ImageSizingMode::ShrinkOnly,
                enable_animated_images: false,
                ..AppSettings::default()
            },
            &ctx,
        );
        let container = egui::vec2(200.0, 200.0);
        let tex = reader.textures[0].clone().unwrap();
        assert_eq!(
            reader.image_draw_size(&tex, container, 1.0),
            egui::vec2(40.0, 60.0)
        );
        reader.next_page(&ctx, false);
        let tex = reader.textures[0].clone().unwrap();
        let size = reader.image_draw_size(&tex, container, 1.0);
        assert!((size.y - 200.0).abs() < 1e-3 && size.x < 200.0);
    }

    #[test]
    fn reopening_a_book_resumes_at_its_bookmark() {
        let parent = TestPages::new(&[]);
        for book in ["a", "b"] {
            let dir = parent.0.join(book);
            fs::create_dir(&dir).unwrap();
            for index in 0..6 {
                image::RgbaImage::new(10, 20)
                    .save(dir.join(format!("{index:03}.png")))
                    .unwrap();
            }
        }
        let (a, b) = (parent.0.join("a"), parent.0.join("b"));
        let ctx = egui::Context::default();
        // history_path が None なので、履歴はファイルに保存されない
        let mut reader = MangaReader::with_settings(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            None,
        );
        reader.open_source(a.clone(), &ctx);
        assert_eq!(reader.current_index, 0);
        reader.go_to_page(3, &ctx);

        // 別の冊に移ると、離れた冊の位置が記録される
        reader.open_source(b.clone(), &ctx);
        assert_eq!(reader.current_index, 0);
        assert_eq!(reader.history.entries[0].path, history::normalize(&a));
        assert_eq!(reader.history.entries[0].page_index, 3);

        // 開き直すとしおりのページから
        reader.open_source(a.clone(), &ctx);
        assert_eq!(reader.current_index, 3);

        // 冊の移動（次のファイル）はしおりを使わず先頭から
        reader.next_zip(&ctx);
        assert_eq!(reader.zip_path, Some(b.clone()));
        assert_eq!(reader.current_index, 0);

        // 画像を指定して開いたときは、しおりよりその画像を優先する
        reader.open_source(a.join("001.png"), &ctx);
        assert_eq!(reader.current_index, 1);

        for book in ["a", "b"] {
            fs::remove_dir_all(parent.0.join(book)).unwrap();
        }
    }

    #[test]
    fn folders_with_subfolders_ask_once_and_remember_the_choice() {
        use crate::config::SubfolderMode;
        // 直下に2枚、サブフォルダ vol1 に1枚
        let pages = TestPages::new(&[[10, 20], [10, 20]]);
        fs::create_dir(pages.0.join("vol1")).unwrap();
        image::RgbaImage::new(10, 20)
            .save(pages.0.join("vol1/000.png"))
            .unwrap();
        let ctx = egui::Context::default();
        let reader = |mode| {
            MangaReader::with_settings(
                AppSettings {
                    page_view_options: PageViewOptions::Single,
                    enable_animated_images: false,
                    resize_method: ResizeMethod::None,
                    subfolder_mode: mode,
                    ..AppSettings::default()
                },
                None,
            )
        };

        // 確認する設定では、答えるまで開かない。キャンセルなら開かない
        let mut asking = reader(SubfolderMode::Ask);
        asking.open_source(pages.0.clone(), &ctx);
        assert!(asking.subfolder_question.is_some());
        assert_eq!(asking.zip_path, None);
        asking.answer_subfolder_question(None, &ctx);
        assert!(asking.subfolder_question.is_none());
        assert_eq!(asking.zip_path, None);

        // 「サブフォルダも読む」と答えると、サブフォルダのページも入る
        asking.open_source(pages.0.clone(), &ctx);
        asking.answer_subfolder_question(Some(true), &ctx);
        assert_eq!(asking.zip_path, Some(pages.0.clone()));
        assert_eq!(asking.image_files.len(), 3);
        assert!(asking.image_files[2].ends_with("vol1/000.png"));

        // 覚えた選択に従い、次からは聞かない
        asking.open_source(pages.0.clone(), &ctx);
        assert!(asking.subfolder_question.is_none());
        assert_eq!(asking.image_files.len(), 3);

        // 覚えないと答えたときは、次もまた聞く
        let mut forgetful = reader(SubfolderMode::Ask);
        forgetful.open_source(pages.0.clone(), &ctx);
        forgetful.subfolder_question.as_mut().unwrap().remember = false;
        forgetful.answer_subfolder_question(Some(false), &ctx);
        assert_eq!(forgetful.image_files.len(), 2);
        forgetful.open_source(pages.0.clone(), &ctx);
        assert!(forgetful.subfolder_question.is_some());

        // 読む／読まないの設定では聞かない
        for (mode, count) in [(SubfolderMode::Include, 3), (SubfolderMode::Exclude, 2)] {
            let mut fixed = reader(mode);
            fixed.open_source(pages.0.clone(), &ctx);
            assert!(fixed.subfolder_question.is_none());
            assert_eq!(fixed.image_files.len(), count, "{mode:?}");
        }

        fs::remove_dir_all(pages.0.join("vol1")).unwrap();
    }

    #[test]
    fn opening_a_pdf_never_panics() {
        // Holds without a pdfium library (it reports that) and with one (the
        // file is not a valid PDF).
        let pages = TestPages::new(&[]);
        let pdf = pages.0.join("book.pdf");
        fs::write(&pdf, b"not a pdf").unwrap();
        let ctx = egui::Context::default();
        let mut reader = MangaReader::with_settings(AppSettings::default(), None);
        reader.load_source(pdf, false, &ctx);
        assert!(reader.image_files.is_empty());
        assert!(reader.error_msg.is_some());
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
    fn repeat_mode_cycles_off_loop_folder() {
        let order = [
            LastPageAction::Nothing,
            LastPageAction::ToFirstPage,
            LastPageAction::GotoNextFile,
        ];
        for (index, action) in order.iter().enumerate() {
            assert_eq!(action.next_in_cycle(), order[(index + 1) % order.len()]);
        }
    }

    #[test]
    fn toolbar_goes_to_the_bottom_unless_chosen_otherwise() {
        // 既存の設定ファイルに項目が無くても、ミーヤと同じく下に置く
        let mut json = serde_json::to_value(AppSettings::default()).unwrap();
        json.as_object_mut().unwrap().remove("toolbar_at_bottom");
        let config: AppSettings = serde_json::from_value(json).unwrap();
        assert!(config.toolbar_at_bottom);
        // 上を選んだら上のまま
        let mut json = serde_json::to_value(AppSettings::default()).unwrap();
        json["toolbar_at_bottom"] = serde_json::Value::Bool(false);
        let config: AppSettings = serde_json::from_value(json).unwrap();
        assert!(!config.toolbar_at_bottom);
    }

    #[test]
    fn settings_without_repeat_key_default_to_f8() {
        let mut json = serde_json::to_value(AppSettings::default()).unwrap();
        for keys in ["keys", "top_down_keys"] {
            json[keys]
                .as_object_mut()
                .unwrap()
                .remove("cycle_last_page_action");
        }
        let config: AppSettings = serde_json::from_value(json).unwrap();
        // Shortcut は Debug を持たないので、比較できる組に直す
        let parts = |s: Option<Shortcut>| s.map(|s| (s.key, s.ctrl, s.alt, s.shift));
        let f8 = Some((egui::Key::F8, false, false, false));
        assert_eq!(parts(config.keys.cycle_last_page_action), f8);
        assert_eq!(parts(config.top_down_keys.cycle_last_page_action), f8);

        // 利用者が割り当てを外した（null の）ときは外したままにする
        let mut json = serde_json::to_value(AppSettings::default()).unwrap();
        json["keys"]["cycle_last_page_action"] = serde_json::Value::Null;
        let config: AppSettings = serde_json::from_value(json).unwrap();
        assert_eq!(parts(config.keys.cycle_last_page_action), None);
    }

    #[test]
    fn mangameeya_key_preset_matches_the_spec_without_conflicts() {
        use crate::config::KeyConfig;
        let keys = KeyConfig::mangameeya();
        let parts = |s: Option<Shortcut>| s.map(|s| (s.key, s.ctrl, s.alt, s.shift));
        let key = |k, shift| Some((k, false, false, shift));
        // ミーヤのマンガ用設定に近いキー配置
        assert_eq!(parts(keys.next_page), key(egui::Key::ArrowRight, false));
        assert_eq!(parts(keys.prev_page), key(egui::Key::ArrowLeft, false));
        assert_eq!(parts(keys.one_next_page), key(egui::Key::ArrowDown, false));
        assert_eq!(parts(keys.one_prev_page), key(egui::Key::ArrowUp, false));
        assert_eq!(parts(keys.last_page), key(egui::Key::End, false));
        assert_eq!(parts(keys.first_page), key(egui::Key::Home, false));
        assert_eq!(parts(keys.fullscreen), key(egui::Key::Enter, false));
        assert_eq!(parts(keys.next_file), key(egui::Key::ArrowRight, true));
        assert_eq!(parts(keys.prev_file), key(egui::Key::ArrowLeft, true));
        assert_eq!(
            parts(keys.cycle_last_page_action),
            key(egui::Key::F8, false)
        );
        assert_eq!(
            parts(keys.toggle_auto_page_layout),
            key(egui::Key::Space, false)
        );
        let alt = |k| Some((k, false, true, false));
        assert_eq!(parts(keys.jump_forward), alt(egui::Key::ArrowRight));
        assert_eq!(parts(keys.jump_backward), alt(egui::Key::ArrowLeft));
        assert_eq!(parts(keys.thumbnails_fit_all), key(egui::Key::A, false));
        assert_eq!(parts(keys.thumbnails_large), key(egui::Key::H, false));
        assert_eq!(
            parts(keys.cover_thumbnails),
            key(egui::Key::ArrowDown, true)
        );
        assert_eq!(parts(keys.upper_folder), key(egui::Key::ArrowUp, true));

        // 同じキーに2つの操作が割り当てられていないこと
        let all = [
            keys.slide_image_down,
            keys.slide_image_up,
            keys.toggle_auto_scroll,
            keys.reload_current_image,
            keys.next_page,
            keys.prev_page,
            keys.one_next_page,
            keys.one_prev_page,
            keys.first_page,
            keys.last_page,
            keys.next_file,
            keys.prev_file,
            keys.next_folder,
            keys.prev_folder,
            keys.fullscreen,
            keys.view_mode,
            keys.open_file,
            keys.quit_app,
            keys.cycle_last_page_action,
            keys.scroll_next_page,
            keys.scroll_prev_page,
            keys.scroll_up,
            keys.scroll_down,
            keys.scroll_left,
            keys.scroll_right,
            keys.toggle_loupe,
            keys.toggle_auto_page_layout,
            keys.jump_forward,
            keys.jump_backward,
            keys.toggle_split_pages,
            keys.thumbnails_fit_all,
            keys.thumbnails_large,
            keys.cover_thumbnails,
            keys.upper_folder,
        ];
        let assigned: Vec<_> = all.into_iter().filter_map(parts).collect();
        for (index, shortcut) in assigned.iter().enumerate() {
            assert!(
                !assigned[index + 1..].contains(shortcut),
                "{shortcut:?} が重複している"
            );
        }
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
        // 最後の1枚は白紙と並ぶ
        assert_view(&reader, 2, PageViewOptions::DoubleRL, 2);
        assert!(reader.is_cover_blank(&reader.textures[1]));

        reader.config.auto_page_layout_rl = true;
        reader.config.page_view_options = PageViewOptions::TopDown;
        let pair = reader.load_pair(0, &ctx);
        assert!(pair.iter().all(Option::is_some));
        assert_eq!(reader.automatic_page_direction(), None);
    }
}
