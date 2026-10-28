// このファイルは RustMangaReader（Lycoris52、Apache License 2.0）のコードを元に改変したものです。

mod animation;
mod book_search;
mod commands;
mod input;
mod loader;
mod menubar;
mod navigation;
mod overlay;
mod pageview;
mod plugins;
mod settings_panel;
mod sized_cache;
mod thumbnails;
mod toolbar;
mod view;

use crate::config::{AppSettings, MangaAction, SourceMode};
use crate::engine::history::History;
use crate::engine::pdf::PdfDoc;
use crate::engine::settings_store;
use crate::font;
use crate::localize::{set_language, tr};
use animation::{AnimatedFrame, AnimatedPage, AnimationScanEvent, AnimationScanStatus};
use eframe::egui;
use gilrs::Gilrs;
use settings_panel::{BindingTarget, ControlProfile};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

const LONG_CLICK_DURATION: Duration = Duration::from_millis(450);
const GAMEPAD_INITIAL_REPEAT_DELAY: Duration = Duration::from_millis(800);
const MOUSE_DRAG_THRESHOLD: f32 = 6.0;
const AUTO_SCROLL_MIN_DELAY_MS: u64 = 200;
const AUTO_SCROLL_MAX_DELAY_MS: u64 = 30_000;
const ANIMATION_SCAN_AHEAD_PAGES: usize = 5;
/// 読んだページのテクスチャをためておく大きさの合計の上限（バイト）。超えたら長く見ていないページから捨てる
const TEXTURE_CACHE_BYTES: usize = 1024 * 1024 * 1024;
/// アニメーションの元データと最初のフレームを、それぞれためておく大きさの合計の上限（バイト）
const ANIMATION_CACHE_BYTES: usize = 256 * 1024 * 1024;
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
/// メニューバーの高さ。全画面で上端に近づけたときに出す範囲に使う
const MENU_BAR_HEIGHT: f32 = 24.0;

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
    texture_cache: sized_cache::SizedCache<egui::TextureHandle>,
    image_dimensions: std::collections::HashMap<String, [u32; 2]>,
    /// いまタイトルバーに出している文字
    window_title: String,
    /// 表紙サムネイルの検索の語
    shelf_filter: String,
    /// 検索でしぼって見えなくなった本の表紙（しぼりを緩めたときに作り直さないように）
    shelf_texture_stash: std::collections::HashMap<PathBuf, egui::TextureHandle>,
    /// 次のフレームで検索の欄に移る（Ctrl+F）
    focus_shelf_search: bool,
    /// IME で、確定前の入力（Preedit）が、前の確定のあとにあったか
    ime_had_preedit: bool,
    /// 本の中の検索（開いていれば）
    book_search: Option<book_search::BookSearch>,
    /// OCR で読んでいる本（検索の窓を閉じても続ける）
    book_ocr: Option<book_search::BookOcr>,
    active_animations: [Option<AnimatedPage>; 2],
    active_animation_keys: [Option<String>; 2],
    animated_image_byte_cache: sized_cache::SizedCache<Vec<u8>>,
    animated_image_first_frame_cache: sized_cache::SizedCache<AnimatedFrame>,
    animation_scan_anchor: Option<usize>,
    animation_scan_generation: u64,
    animation_scan_epoch: Arc<AtomicU64>,
    animation_scan_rx: Option<Receiver<AnimationScanEvent>>,
    animation_scan_status: std::collections::HashMap<String, AnimationScanStatus>,
    pdf_document: Option<PdfDoc>,
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
    /// 表紙サムネイルで押された Tab（Shift つきなら true）。egui のフォーカス移動に渡さず取っておく
    pending_shelf_tab: Option<bool>,
    /// 表紙サムネイルで最後に並べたフォルダ
    last_shelf_folder: Option<PathBuf>,
    /// 表紙サムネイルで前に並べていたものと、そのとき選んでいた冊。新しいものが末尾
    shelf_back: Vec<(thumbnails::ShelfSource, Option<PathBuf>)>,
    /// 戻るで一覧を移っているあいだ（そのときは道のりに足さない）
    going_back: bool,
    /// ブックリストのプラグイン
    plugins: plugins::PluginHost,
    /// 次に表示し直すとき、見開きの組を合わせ直さない（サムネイルで選んだページから見開きにする）
    keep_spread_start: bool,
    /// 開いているメニューと、メニューから開いたダイアログ
    menu_state: menubar::MenuState,
    menu_dialog: Option<menubar::MenuDialog>,
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
        reader.plugins = plugins::PluginHost::discover();
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
            texture_cache: sized_cache::SizedCache::new(TEXTURE_CACHE_BYTES),
            image_dimensions: Default::default(),
            window_title: String::new(),
            shelf_filter: String::new(),
            shelf_texture_stash: Default::default(),
            focus_shelf_search: false,
            ime_had_preedit: false,
            book_search: None,
            book_ocr: None,
            active_animations: std::array::from_fn(|_| None),
            active_animation_keys: std::array::from_fn(|_| None),
            animated_image_byte_cache: sized_cache::SizedCache::new(ANIMATION_CACHE_BYTES),
            animated_image_first_frame_cache: sized_cache::SizedCache::new(ANIMATION_CACHE_BYTES),
            animation_scan_anchor: None,
            animation_scan_generation: 0,
            animation_scan_epoch: Arc::new(AtomicU64::new(0)),
            animation_scan_rx: None,
            animation_scan_status: Default::default(),
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
            fullscreen_top_chrome_visible: false,
            fullscreen_bottom_toolbar_visible: false,
            history: History::default(),
            history_path: None,
            subfolder_question: None,
            manual_page_view: false,
            split_second_half_of: None,
            thumbnails: None,
            pending_shelf_tab: None,
            last_shelf_folder: None,
            shelf_back: Vec::new(),
            going_back: false,
            plugins: plugins::PluginHost::with(
                Vec::new(),
                crate::engine::credentials::Credentials::memory_only(),
                std::env::temp_dir().join("mangareeda-test-cache"),
            ),
            thumbnail_cache: Default::default(),
            keep_spread_start: false,
            menu_state: Default::default(),
            menu_dialog: None,
        }
    }

    fn save_settings(&self) {
        settings_store::save(&self.config);
    }

    fn show_fading_error(&mut self, msg: &str) {
        self.error_msg = Some((msg.to_string(), Instant::now()));
    }
}
/// 確定前の入力（Preedit）が無いまま届いた IME の確定を、ふつうの文字の入力にする。
/// egui の入力欄は、確定前の入力があった位置と今のカーソルが同じときだけ確定を入れるので、
/// 日本語入力のまま押した空白（fcitx5 などは、確定前の入力なしにすぐ確定する）が捨てられてしまうため
fn fix_bare_ime_commits(events: &mut [egui::Event], had_preedit: &mut bool) {
    for event in events.iter_mut() {
        match event {
            // 空の確定前の入力（何も入力していないときにも届く）は数えない
            egui::Event::Ime(egui::ImeEvent::Preedit(text)) if !text.is_empty() => {
                *had_preedit = true
            }
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                if !std::mem::take(had_preedit) && !text.is_empty() {
                    *event = egui::Event::Text(std::mem::take(text));
                }
            }
            egui::Event::Ime(egui::ImeEvent::Disabled) => *had_preedit = false,
            _ => {}
        }
    }
}

impl eframe::App for MangaReader {
    /// 表紙サムネイルを開いているあいだは、Tab をタブの切り替えに使う。
    /// egui に渡すとメニューやツールバーにフォーカスが移ってしまうので、ここで取り除く
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        fix_bare_ime_commits(&mut raw_input.events, &mut self.ime_had_preedit);
        if self.shelf_source().is_none() {
            return;
        }
        raw_input.events.retain(|event| match event {
            egui::Event::Key {
                key: egui::Key::Tab,
                pressed,
                modifiers,
                ..
            } => {
                if *pressed {
                    self.pending_shelf_tab = Some(modifiers.shift);
                }
                false
            }
            _ => true,
        });
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        set_language(self.config.language);
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

        // 検索の窓が開いているあいだの Esc は、窓を閉じるだけにする（終了などのキー操作に渡さない）
        self.close_book_search_on_escape(ctx);
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
        self.poll_plugins(ctx);

        // メニューのキー操作は、ほかの操作より先に受け取る
        let menu_was_open = self.menu_state.is_open();
        let mut menu_command = self.handle_menu_keys(ctx);

        self.update_fullscreen_top_chrome_visibility(ctx);
        if self.is_fullscreen && self.fullscreen_top_chrome_visible {
            self.render_fullscreen_title_bar(ctx);
        }
        // 全画面では、上端に近づけたときとメニューを開いているときだけ出す
        let show_menu_bar =
            !self.is_fullscreen || self.fullscreen_top_chrome_visible || self.menu_state.is_open();
        if show_menu_bar && let Some(command) = self.render_menu_bar(ctx) {
            menu_command = Some(command);
        }
        // メニューやダイアログを使っているあいだは、ページの操作を受け付けない
        if menu_was_open
            || self.menu_state.is_open()
            || menu_command.is_some()
            || self.menu_dialog.is_some()
        {
            action_to_run = MangaAction::None;
        }

        self.render_settings_panel(ctx);
        self.render_toolbar(ctx);
        self.tick_auto_scroll(ctx, action_to_run);

        // 文字を入力している（検索の欄や設定の欄にいる）あいだは、キー操作を受け付けない
        if ctx.wants_keyboard_input() {
            action_to_run = MangaAction::None;
        }
        if self.thumbnails.is_some() {
            // 一覧を開いているあいだは、一覧の切り替え以外の操作を受け付けない
            if !matches!(
                action_to_run,
                MangaAction::ThumbnailsFitAll
                    | MangaAction::ThumbnailsLarge
                    | MangaAction::CoverThumbnails
                    | MangaAction::UpperFolder
                    | MangaAction::ShowHistory
                    | MangaAction::ShowFavorites
            ) {
                action_to_run = MangaAction::None;
            }
            self.render_thumbnails(ctx);
        } else {
            self.render_page_view(ctx, &mut action_to_run);
        }
        // 読んでいるときの Ctrl+F で、本の中の検索を開く
        if self.thumbnails.is_none()
            && !ctx.wants_keyboard_input()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F))
        {
            self.open_book_search();
        }
        self.poll_book_ocr(ctx);
        if self.thumbnails.is_none() {
            self.render_book_search(ctx);
        }
        self.render_subfolder_question(ctx);
        self.render_menu_dialog(ctx);
        // ダイアログに答えるまでは、ページの操作を受け付けない
        if self.subfolder_question.is_some() || self.menu_dialog.is_some() {
            action_to_run = MangaAction::None;
        }
        if let Some(command) = menu_command {
            self.execute_menu_command(command, ctx);
        }

        if action_to_run != MangaAction::None {
            self.execute_action(action_to_run, ctx);
        }

        // Keep preloading buffers
        self.update_buffers(ctx);

        self.tidy_closed_shelf();

        // タイトルバー。変わったときだけ送る
        let title = self.window_title(ctx);
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.remember_current_book();
        self.save_settings();
        // OCR のプラグインを止め、作業用の画像を消し終えるのを待つ
        if let Some(ocr) = self.book_ocr.take() {
            ocr.stop_and_wait(std::time::Duration::from_secs(5));
            // 待ちきれなかったときも、作業用の画像は残さない
            crate::engine::book_text::remove_ocr_work(&crate::engine::settings_store::cache_dir());
        }
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
            let removed = fs::remove_dir_all(&self.0);
            // テストが失敗して片付けの途中なら、ここでは止めない（二重のパニックでテスト全体が止まらないように）
            if !std::thread::panicking() {
                removed.unwrap();
            }
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
    fn history_and_favorites_are_listed_with_covers() {
        use super::thumbnails::{ShelfSource, ShelfTab};
        use std::io::Write;
        let shelf = TestPages::new(&[]);
        let write_zip = |path: &PathBuf| {
            let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
            zip.start_file("01.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            let mut bytes = Vec::new();
            image::RgbaImage::new(10, 20)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            zip.write_all(&bytes).unwrap();
            zip.finish().unwrap();
        };
        let a = history::normalize(&shelf.0).join("A.zip");
        let b = history::normalize(&shelf.0).join("B.zip");
        write_zip(&a);
        write_zip(&b);

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
        // 履歴に無い冊（消したもの）は並べない
        reader
            .history
            .record(&shelf.0.join("gone.zip"), &["01.png".to_owned()], 0);
        reader.open_source(a.clone(), &ctx);
        reader.open_source(b.clone(), &ctx);
        reader.remember_current_book();

        // 新しく読んだ順。今の冊が選ばれている
        reader.toggle_book_list(ShelfSource::History);
        let view = reader.thumbnails.as_ref().unwrap();
        assert_eq!(view.cover_paths(), [b.clone(), a.clone()]);
        assert_eq!(view.selected(), 0);

        // 履歴から外しても、ファイルは消さない
        reader.thumbnails.as_mut().unwrap().select(1);
        reader.remove_selected_from_shelf();
        assert_eq!(
            reader.thumbnails.as_ref().unwrap().cover_paths(),
            std::slice::from_ref(&b)
        );
        assert!(a.exists());

        // お気に入りは空でも開く。登録すると並ぶ
        reader.switch_shelf_tab(ShelfTab::Favorites);
        assert_eq!(reader.shelf_source(), Some(ShelfSource::Favorites));
        assert!(reader.thumbnails.as_ref().unwrap().cover_paths().is_empty());
        reader.toggle_current_favorite();
        reader.toggle_book_list(ShelfSource::Favorites);
        assert!(reader.thumbnails.is_none());
        reader.toggle_book_list(ShelfSource::Favorites);
        assert_eq!(
            reader.thumbnails.as_ref().unwrap().cover_paths(),
            std::slice::from_ref(&b)
        );

        // フォルダのタブは、今の冊が入っているフォルダ
        reader.switch_shelf_tab(ShelfTab::Folder);
        assert_eq!(
            reader.thumbnails.as_ref().unwrap().cover_paths(),
            [a.clone(), b.clone()]
        );

        // 選んだ冊を開く
        reader.toggle_book_list(ShelfSource::History);
        reader.choose_thumbnail(0, &ctx);
        assert_eq!(reader.zip_path, Some(b.clone()));

        // 冊を開いていなくて、お気に入りも空でも、フォルダのタブに移れる（最後に読んだ冊のフォルダ）
        reader.close_book();
        reader.last_shelf_folder = None;
        reader.history.favorites.clear();
        reader.toggle_book_list(ShelfSource::Favorites);
        reader.switch_shelf_tab(ShelfTab::Folder);
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(history::normalize(&shelf.0)))
        );

        // Tab は egui に渡さず、タブの切り替えに使う
        use eframe::App as _;
        let tab = |shift| egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                shift,
                ..Default::default()
            },
        };
        let mut input = egui::RawInput {
            events: vec![tab(true)],
            ..Default::default()
        };
        reader.raw_input_hook(&ctx, &mut input);
        assert!(input.events.is_empty());
        let _ = ctx.run(Default::default(), |ctx| reader.render_thumbnails(ctx));
        assert_eq!(reader.shelf_source(), Some(ShelfSource::Favorites));

        for path in [a, b] {
            fs::remove_file(path).unwrap();
        }
    }

    /// 表紙サムネイルは、検索の語で名前をしぼれる（大文字・小文字、全角・半角を区別せず、空白で区切った語をすべて含むもの）
    #[test]
    fn cover_thumbnails_can_be_narrowed_by_name() {
        use super::thumbnails::{ShelfSource, ShelfTab};
        use std::io::Write;
        let root = TestPages::new(&[]);
        let root_path = history::normalize(&root.0);
        let mut png = Vec::new();
        image::RgbaImage::new(10, 20)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let names = [
            "Nausicaa vol1.zip",
            "Nausicaa vol2.zip",
            "風の谷 第03巻.zip",
            "other.zip",
        ];
        for name in names {
            let mut zip = zip::ZipWriter::new(fs::File::create(root_path.join(name)).unwrap());
            zip.start_file("01.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&png).unwrap();
            zip.finish().unwrap();
        }
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
        reader.open_source(root_path.join("other.zip"), &ctx);
        reader.toggle_cover_thumbnails();
        let shown = |reader: &MangaReader| -> Vec<String> {
            reader
                .thumbnails
                .as_ref()
                .unwrap()
                .cover_paths()
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(shown(&reader).len(), 4);

        let narrow = |reader: &mut MangaReader, query: &str| {
            reader.shelf_filter = query.to_owned();
            reader.reshow_shelf();
            shown(reader)
        };
        // 全角の英字でも、大文字・小文字が違っても合う
        assert_eq!(
            narrow(&mut reader, "ＮＡＵ"),
            ["Nausicaa vol1.zip", "Nausicaa vol2.zip"]
        );
        // 空白で区切った語をすべて含むもの
        assert_eq!(narrow(&mut reader, "nausicaa 2"), ["Nausicaa vol2.zip"]);
        assert_eq!(narrow(&mut reader, "第03"), ["風の谷 第03巻.zip"]);
        assert!(narrow(&mut reader, "none").is_empty());
        // 検索を消すと、全部に戻る
        assert_eq!(narrow(&mut reader, "").len(), 4);

        // 検索したまま子のフォルダに入ると、子のフォルダの本は前の語でしぼらずに全部出る
        let sub = root_path.join("sub");
        fs::create_dir(&sub).unwrap();
        for name in ["x.zip", "y.zip"] {
            let mut zip = zip::ZipWriter::new(fs::File::create(sub.join(name)).unwrap());
            zip.start_file("01.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&png).unwrap();
            zip.finish().unwrap();
        }
        assert_eq!(narrow(&mut reader, "sub"), ["sub"]);
        reader.choose_thumbnail(0, &ctx);
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(sub.clone()))
        );
        assert!(reader.shelf_filter.is_empty());
        assert_eq!(shown(&reader), ["x.zip", "y.zip"]);
        reader.go_back_in_shelf();
        fs::remove_dir_all(&sub).unwrap();
        reader.reshow_shelf();

        // 別の並びに移ったら、検索は消える
        narrow(&mut reader, "nausicaa");
        reader.switch_shelf_tab(ShelfTab::History);
        assert!(reader.shelf_filter.is_empty());
        reader.switch_shelf_tab(ShelfTab::Folder);
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(root_path.clone()))
        );
        assert_eq!(shown(&reader).len(), 4);

        // キーで: Ctrl+F で検索の欄に移り、打った文字でしぼる
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let frame_with =
            |reader: &mut MangaReader, events: Vec<egui::Event>, modifiers: egui::Modifiers| {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    )),
                    events,
                    modifiers,
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| reader.render_thumbnails(ctx));
            };
        let frame = |reader: &mut MangaReader, events: Vec<egui::Event>| {
            frame_with(reader, events, egui::Modifiers::NONE)
        };
        frame_with(
            &mut reader,
            vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
            egui::Modifiers::COMMAND,
        );
        frame(&mut reader, vec![]);
        frame(&mut reader, vec![egui::Event::Text("vol".to_owned())]);
        assert_eq!(reader.shelf_filter, "vol");
        assert_eq!(shown(&reader), ["Nausicaa vol1.zip", "Nausicaa vol2.zip"]);
        // 入力中の Backspace は文字を消すだけで、上のフォルダには移らない
        frame(
            &mut reader,
            vec![key(egui::Key::Backspace, egui::Modifiers::NONE)],
        );
        assert_eq!(reader.shelf_filter, "vo");
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(root_path.clone()))
        );
        let escape = || vec![key(egui::Key::Escape, egui::Modifiers::NONE)];
        // 日本語入力のまま押した空白（確定前の入力なしに届く確定）も、区切りとして入る
        let mut events = vec![egui::Event::Ime(egui::ImeEvent::Commit(
            "\u{3000}".to_owned(),
        ))];
        let mut had_preedit = false;
        fix_bare_ime_commits(&mut events, &mut had_preedit);
        assert_eq!(events, [egui::Event::Text("\u{3000}".to_owned())]);
        let mut events = vec![
            egui::Event::Ime(egui::ImeEvent::Preedit(String::new())),
            egui::Event::Ime(egui::ImeEvent::Commit(" ".to_owned())),
        ];
        fix_bare_ime_commits(&mut events, &mut had_preedit);
        assert_eq!(events[1], egui::Event::Text(" ".to_owned()));
        // 変換した文字（確定前の入力のあとの確定）は、そのまま egui に任せる
        let mut events = vec![
            egui::Event::Ime(egui::ImeEvent::Preedit("かぜ".to_owned())),
            egui::Event::Ime(egui::ImeEvent::Commit("風".to_owned())),
        ];
        fix_bare_ime_commits(&mut events, &mut had_preedit);
        assert_eq!(
            events[1],
            egui::Event::Ime(egui::ImeEvent::Commit("風".to_owned()))
        );
        assert!(!had_preedit);
        // 入力中の ↓ で、しぼった一覧に移る。そこでは ←→ で選び、Enter で開く
        frame(&mut reader, vec![egui::Event::Text("l".to_owned())]);
        assert_eq!(reader.shelf_filter, "vol");
        assert_eq!(reader.thumbnails.as_ref().unwrap().selected(), 0);
        frame(
            &mut reader,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        frame(
            &mut reader,
            vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)],
        );
        assert_eq!(reader.thumbnails.as_ref().unwrap().selected(), 1);
        assert_eq!(reader.shelf_filter, "vol");
        // 欄を離れたあとの文字は、検索に入らない
        frame(&mut reader, vec![egui::Event::Text("x".to_owned())]);
        assert_eq!(reader.shelf_filter, "vol");
        // 検索に戻って、Esc で検索を消す
        frame_with(
            &mut reader,
            vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
            egui::Modifiers::COMMAND,
        );
        frame(&mut reader, vec![]);
        // Esc は検索を消して欄を離れ、一覧は閉じない
        frame(&mut reader, escape());
        assert!(reader.shelf_filter.is_empty());
        assert_eq!(shown(&reader).len(), 4);
        assert!(reader.thumbnails.is_some());
        // 空の欄にいるときの Esc も、欄を離れるだけ
        frame_with(
            &mut reader,
            vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
            egui::Modifiers::COMMAND,
        );
        frame(&mut reader, vec![]);
        frame(&mut reader, escape());
        assert!(reader.thumbnails.is_some());
        // 欄にいなければ、Esc で一覧を閉じる
        frame(&mut reader, escape());
        assert!(reader.thumbnails.is_none());

        // 入力して Enter で一覧に移り、もう一度 Enter で選んだ本を開く
        reader.toggle_cover_thumbnails();
        frame_with(
            &mut reader,
            vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
            egui::Modifiers::COMMAND,
        );
        frame(&mut reader, vec![]);
        frame(&mut reader, vec![egui::Event::Text("vol2".to_owned())]);
        frame(
            &mut reader,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert!(reader.thumbnails.is_some());
        frame(
            &mut reader,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert!(reader.thumbnails.is_none());
        assert_eq!(reader.zip_path, Some(root_path.join("Nausicaa vol2.zip")));

        // 検索で取っておいた表紙は、一覧を閉じたら捨てる
        reader.shelf_filter = "vol2".to_owned();
        reader.shelf_texture_stash.insert(
            root_path.join("other.zip"),
            ctx.load_texture(
                "stashed",
                egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                egui::TextureOptions::default(),
            ),
        );
        reader.tidy_closed_shelf();
        assert!(reader.shelf_texture_stash.is_empty());
        assert!(reader.shelf_filter.is_empty());
    }

    /// 本の中の検索は、文字の層の無い本では探せないと知らせ、別の本に移ったら閉じる
    #[test]
    fn book_search_tells_when_there_is_no_text_and_closes_on_another_book() {
        let first = TestPages::new(&[[10, 20], [10, 20]]);
        let second = TestPages::new(&[[10, 20]]);
        let ctx = egui::Context::default();
        let mut reader = first.reader(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        let frame = |reader: &mut MangaReader| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                reader.poll_book_ocr(ctx);
                reader.render_book_search(ctx)
            });
        };
        reader.open_book_search();
        frame(&mut reader);
        let search = reader.book_search.as_mut().unwrap();
        search.query = "語".to_owned();
        frame(&mut reader);
        assert!(reader.book_search.as_ref().unwrap().hits.is_empty());
        // 別の本に移ったら閉じる
        reader.open_source(second.0.clone(), &ctx);
        frame(&mut reader);
        assert!(reader.book_search.is_none());
    }

    /// 文字の層の無い本は、OCR のプラグインで読み、探せるようになる。検索の窓を閉じても読み続ける
    #[cfg(unix)]
    #[test]
    fn books_without_text_can_be_read_with_ocr_even_after_closing_the_search_window() {
        use std::os::unix::fs::PermissionsExt;
        let pages = TestPages::new(&[[10, 20], [10, 20], [10, 20]]);
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
        let plugin_dir = pages.0.join("plugin");
        fs::create_dir(&plugin_dir).unwrap();
        let script = plugin_dir.join("ocr.sh");
        fs::write(
            &script,
            "#!/bin/sh\nread request\n\
             for page in $(echo \"$request\" | grep -o '\"page\":[0-9]*' | cut -d: -f2); do\n\
             echo \"{\\\"page\\\": $page, \\\"text\\\": \\\"ページ$page\\\"}\"\n\
             done\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        reader.plugins = super::plugins::PluginHost::with(
            vec![crate::engine::plugin::PluginManifest {
                id: "fake".to_owned(),
                name: "Fake OCR".to_owned(),
                command: vec!["./ocr.sh".to_owned()],
                credentials: Vec::new(),
                auth: false,
                kind: crate::engine::plugin::PluginKind::Ocr,
                ocr_batch: None,
                dir: plugin_dir.clone(),
            }],
            crate::engine::credentials::Credentials::memory_only(),
            pages.0.join("cache"),
        );
        // OCR のプラグインは、本の一覧のタブには並べない
        assert!(reader.plugins.manifests.is_empty());
        assert_eq!(reader.ocr_engine().map(|e| e.id.as_str()), Some("fake"));
        let frame = |reader: &mut MangaReader| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                reader.poll_book_ocr(ctx);
                reader.render_book_search(ctx)
            });
        };
        reader.open_book_search();
        reader.start_book_ocr();
        // 窓を閉じても、OCR は続き、読んだ文字はためておかれる
        reader.book_search = None;
        for _ in 0..300 {
            frame(&mut reader);
            if !reader.book_ocr_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!reader.book_ocr_running());
        reader.open_book_search();
        reader.book_search.as_mut().unwrap().query = "ページ1".to_owned();
        for _ in 0..300 {
            frame(&mut reader);
            if !reader.book_search.as_ref().unwrap().hits.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let hits = &reader.book_search.as_ref().unwrap().hits;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        fs::remove_dir_all(&plugin_dir).unwrap();
        let _ = fs::remove_dir_all(pages.0.join("cache"));
    }

    /// 検索の窓が開いているあいだの Esc は、窓を閉じるだけで、終了などのキー操作には渡さない
    #[test]
    fn escape_closes_the_book_search_without_reaching_shortcuts() {
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
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        reader.open_book_search();
        let mut action = MangaAction::None;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![escape.clone()],
                ..Default::default()
            },
            |ctx| {
                reader.close_book_search_on_escape(ctx);
                action = reader.collect_input_action(ctx);
            },
        );
        assert!(reader.book_search.is_none());
        assert_eq!(action, MangaAction::None);
        // 窓が無ければ、Esc はいつもどおりのキー操作になる（既定では終了）
        let _ = ctx.run(
            egui::RawInput {
                events: vec![escape],
                ..Default::default()
            },
            |ctx| {
                reader.close_book_search_on_escape(ctx);
                action = reader.collect_input_action(ctx);
            },
        );
        assert_eq!(action, MangaAction::QuitApp);
    }

    #[test]
    fn folders_with_books_open_as_lists_and_back_returns() {
        use super::thumbnails::ShelfSource;
        use std::io::Write;
        let root = TestPages::new(&[]);
        let root_path = history::normalize(&root.0);
        let png = || {
            let mut bytes = Vec::new();
            image::RgbaImage::new(10, 20)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            bytes
        };
        // 画像と冊が混ざったフォルダ
        let mixed = root_path.join("mixed");
        fs::create_dir(&mixed).unwrap();
        fs::write(mixed.join("01.png"), png()).unwrap();
        let inner = mixed.join("inner.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&inner).unwrap());
        zip.start_file("01.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&png()).unwrap();
        zip.finish().unwrap();

        let ctx = egui::Context::default();
        let mut reader = MangaReader::with_settings(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                subfolder_mode: crate::config::SubfolderMode::Ask,
                ..AppSettings::default()
            },
            None,
        );
        reader.open_source(inner.clone(), &ctx);

        // 直下の画像を読むマスが先頭に付く
        reader.toggle_cover_thumbnails();
        let view = reader.thumbnails.as_ref().unwrap();
        assert_eq!(view.cover_paths(), [mixed.clone(), inner.clone()]);
        assert_eq!(view.selected(), 1);

        // 上に上がって、混ざったフォルダを選ぶと、冊ではなく一覧として開く
        reader.go_to_upper_folder();
        assert_eq!(
            reader.thumbnails.as_ref().unwrap().cover_paths(),
            std::slice::from_ref(&mixed)
        );
        reader.choose_thumbnail(0, &ctx);
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(mixed.clone()))
        );
        assert!(reader.subfolder_question.is_none());

        // 戻ると、上のフォルダで選んでいたフォルダが選ばれている。もう一度で最初の一覧
        reader.go_back_in_shelf();
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(root_path.clone()))
        );
        assert_eq!(reader.thumbnails.as_ref().unwrap().selected(), 0);
        reader.go_back_in_shelf();
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(mixed.clone()))
        );
        assert_eq!(reader.thumbnails.as_ref().unwrap().selected(), 1);
        // それより前は無い
        reader.go_back_in_shelf();
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(mixed.clone()))
        );

        // メニューの「フォルダ」: 履歴を並べていればフォルダに移り、フォルダを並べていれば閉じる
        reader.toggle_book_list(ShelfSource::History);
        reader.toggle_cover_thumbnails();
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Folder(mixed.clone()))
        );
        reader.toggle_cover_thumbnails();
        assert!(reader.thumbnails.is_none());
        reader.toggle_cover_thumbnails();

        // 先頭のマスは、直下の画像だけを冊として開く（サブフォルダは聞かない）
        reader.choose_thumbnail(0, &ctx);
        assert!(reader.thumbnails.is_none());
        assert!(reader.subfolder_question.is_none());
        assert_eq!(reader.zip_path, Some(mixed.clone()));
        assert_eq!(reader.image_files.len(), 1);

        fs::remove_dir_all(mixed).unwrap();
    }

    /// 開けなかったときは、今の冊をそのまま読み続けられる
    #[test]
    fn failed_opens_keep_the_current_book() {
        let ctx = egui::Context::default();
        let pages = TestPages::new(&[[10, 20], [10, 20], [10, 20]]);
        let mut reader = pages.reader(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                ..AppSettings::default()
            },
            &ctx,
        );
        let mode = reader.source_mode;
        let files = reader.image_files.clone();
        // 画像の無いフォルダと、壊れた PDF（pdfium はプロセスで1つを使い回すので、ほかのテストと並べられる）
        let empty = TestPages::new(&[]);
        let broken = empty.0.join("broken.pdf");
        fs::write(&broken, "not a pdf").unwrap();
        for path in [empty.0.clone(), broken] {
            reader.load_source(path, false, &ctx);
            assert_eq!(reader.zip_path, Some(pages.0.clone()));
            assert_eq!(reader.source_mode, mode);
            assert_eq!(reader.image_files, files);
        }
        reader.go_to_page(2, &ctx);
        assert_eq!(reader.current_index, 2);
        assert!(reader.textures[0].is_some());
    }

    /// 名前が同じ別の本は、別の場所に置き、取り違えない
    #[test]
    fn remote_books_with_the_same_name_get_their_own_places() {
        use crate::engine::plugin::RemoteBook;
        let dir = TestPages::new(&[]);
        let library = history::normalize(&dir.0).join("library");
        let mut reader = MangaReader::with_settings(
            AppSettings {
                library_dir: library.to_string_lossy().into_owned(),
                ..AppSettings::default()
            },
            None,
        );
        let book = |id: &str| RemoteBook {
            id: id.to_owned(),
            title: id.to_owned(),
            file_name: "same.pdf".to_owned(),
            cover_url: None,
            folder: false,
            cover_data: None,
        };
        let key = ("p".to_owned(), String::new());
        let mut shelf = super::plugins::PluginShelf::default();
        shelf.books = vec![book("a"), book("b")];
        reader.plugins.shelves.insert(key.clone(), shelf);
        let paths: Vec<_> = reader
            .plugin_entries(&key)
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(paths[0], library.join("p").join("same.pdf"));
        assert_ne!(paths[0], paths[1]);

        // 前に別の本（a）を置いた場所には、b を置かない
        let owner_a = reader.plugins.owner("p", "a");
        reader.record_owner("p", &paths[0], &owner_a);
        let mut shelf = super::plugins::PluginShelf::default();
        shelf.books = vec![book("b")];
        reader.plugins.shelves.insert(key.clone(), shelf);
        let path_b = reader.plugin_entries(&key)[0].path.clone();
        assert_ne!(path_b, paths[0]);
        // 記録はファイルに残り、読み直しても同じ
        reader.plugins.owners.clear();
        reader.load_owners("p");
        assert_eq!(reader.plugin_entries(&key)[0].path, path_b);
        fs::remove_dir_all(library).unwrap();
    }

    /// 別のアカウントの本や、ダウンロード中・待っている別の本の場所は使わない
    #[test]
    fn remote_books_of_other_accounts_and_downloads_keep_their_own_places() {
        use crate::engine::plugin::{PluginManifest, RemoteBook};
        let library = std::env::temp_dir().join(format!("manga-owners2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&library);
        let mut reader = MangaReader::with_settings(
            AppSettings {
                library_dir: library.to_string_lossy().into_owned(),
                ..AppSettings::default()
            },
            None,
        );
        // 起動できないプラグイン（ダウンロードは失敗するが、知らせを受け取るまでは場所を押さえている）
        reader.plugins = super::plugins::PluginHost::with(
            vec![PluginManifest {
                id: "p".to_owned(),
                name: "P".to_owned(),
                command: vec![library.join("missing").to_string_lossy().into_owned()],
                credentials: Vec::new(),
                auth: false,
                kind: crate::engine::plugin::PluginKind::Books,
                ocr_batch: None,
                dir: library.clone(),
            }],
            crate::engine::credentials::Credentials::memory_only(),
            library.join("cache"),
        );
        let book = |id: &str, file_name: &str| RemoteBook {
            id: id.to_owned(),
            title: id.to_owned(),
            file_name: file_name.to_owned(),
            cover_url: None,
            folder: false,
            cover_data: None,
        };
        let set_shelf = |reader: &mut MangaReader, key: &(String, String), books| {
            let mut shelf = super::plugins::PluginShelf::default();
            shelf.books = books;
            reader.plugins.shelves.insert(key.clone(), shelf);
        };
        let top = ("p".to_owned(), String::new());

        // 記録の無いファイル（記録を始める前の版でダウンロードしたもの）は、どの本かわからないので開かない。
        // 別の場所に取り直し、もとのファイルは残す
        let legacy = library.join("p").join("old.pdf");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, "old").unwrap();
        set_shelf(&mut reader, &top, vec![book("o", "old.pdf")]);
        let entry = reader.plugin_entries(&top)[0].clone();
        assert_ne!(entry.path, legacy);
        assert!(!reader.remote_path(&entry).is_file());
        assert!(legacy.is_file());
        // 印を付けた場所にも記録の無いファイルがあれば、そこも使わず、さらに別の印の場所にする
        fs::write(&entry.path, "old marked").unwrap();
        let entry = reader.plugin_entries(&top)[0].clone();
        assert!(!entry.path.exists());
        assert!(!reader.remote_path(&entry).is_file());
        fs::remove_file(&legacy).unwrap();
        // もとの場所が空けば、そこを使う
        assert_eq!(reader.plugin_entries(&top)[0].path, legacy);

        // アカウント A で a をダウンロードしてある
        reader.plugins.set_account(&top, Some("A".to_owned()));
        set_shelf(&mut reader, &top, vec![book("a", "same.pdf")]);
        let path_a = reader.plugin_entries(&top)[0].path.clone();
        assert_eq!(path_a, library.join("p").join("same.pdf"));
        fs::create_dir_all(path_a.parent().unwrap()).unwrap();
        fs::write(&path_a, "A").unwrap();
        let owner_a = reader.plugins.owner("p", "a");
        reader.record_owner("p", &path_a, &owner_a);

        // アカウント B の、id が同じ本は、A の本を開かない
        reader.plugins.set_account(&top, Some("B".to_owned()));
        set_shelf(&mut reader, &top, vec![book("a", "same.pdf")]);
        let entry_b = reader.plugin_entries(&top)[0].clone();
        assert_ne!(entry_b.path, path_a);
        assert!(!reader.remote_path(&entry_b).is_file());
        // A に戻れば、A の本を開ける
        reader.plugins.set_account(&top, Some("A".to_owned()));
        set_shelf(&mut reader, &top, vec![book("a", "same.pdf")]);
        assert_eq!(reader.plugin_entries(&top)[0].path, path_a);

        // 別々のフォルダの、名前が同じ別の本。どちらも、どこも使っていないときに並べた
        let (x, y) = (
            ("p".to_owned(), "x".to_owned()),
            ("p".to_owned(), "y".to_owned()),
        );
        set_shelf(&mut reader, &x, vec![book("x", "dup.pdf")]);
        set_shelf(&mut reader, &y, vec![book("y", "dup.pdf")]);
        let entry_x = reader.plugin_entries(&x)[0].clone();
        let entry_y = reader.plugin_entries(&y)[0].clone();
        assert_eq!(entry_x.path, entry_y.path);
        // x のダウンロードを頼むと、その場所は x のもの。y は別の場所に置き、x の状態を y のものと思わない
        reader.request_download(&entry_x);
        assert!(reader.download_status(&entry_x).is_some());
        assert_eq!(reader.download_status(&entry_y), None);
        assert_ne!(reader.remote_path(&entry_y), entry_x.path);
        reader.request_download(&entry_y);
        assert!(reader.download_status(&entry_y).is_some());
        assert_ne!(reader.plugin_entries(&y)[0].path, entry_x.path);

        // 大文字・小文字を区別しないファイルシステムでは、"Case.pdf" と "case.pdf" は同じ場所。
        // 同じ一覧の中でも、待っている本とも重ねない
        reader.plugins.ignore_case = true;
        let z = ("p".to_owned(), "z".to_owned());
        set_shelf(
            &mut reader,
            &z,
            vec![book("upper", "Case.pdf"), book("lower", "case.pdf")],
        );
        let entries = reader.plugin_entries(&z);
        let key = |path: &PathBuf| path.to_string_lossy().to_lowercase();
        assert_ne!(key(&entries[0].path), key(&entries[1].path));
        reader.request_download(&entries[0]);
        set_shelf(&mut reader, &z, vec![book("lower", "case.pdf")]);
        let lower = reader.plugin_entries(&z)[0].clone();
        assert_ne!(key(&lower.path), key(&entries[0].path));
        // 失敗するダウンロードが終わって（保存先のフォルダを作り終えて）から片付ける
        let ctx = egui::Context::default();
        for _ in 0..500 {
            reader.poll_plugins(&ctx);
            if reader.download_status(&entry_y).is_none()
                && reader.download_status(&entries[0]).is_none()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(reader.download_status(&entry_y), None);
        assert_eq!(reader.download_status(&entries[0]), None);
        fs::remove_dir_all(library).unwrap();
    }

    /// シェルスクリプトで作ったプラグインで、一覧・ダウンロード・開くまでを通す
    #[cfg(unix)]
    #[test]
    fn plugin_books_are_listed_downloaded_and_opened() {
        use super::thumbnails::{ShelfSource, ShelfTab};
        use crate::engine::plugin::PluginManifest;
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = TestPages::new(&[]);
        let root = history::normalize(&dir.0);
        // ダウンロードで渡す書庫
        let mut zip = zip::ZipWriter::new(fs::File::create(root.join("source.zip")).unwrap());
        zip.start_file("01.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        let mut bytes = Vec::new();
        image::RgbaImage::new(10, 20)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        zip.write_all(&bytes).unwrap();
        zip.finish().unwrap();
        let script = root.join("run.sh");
        fs::write(
            &script,
            format!(
                r#"#!/bin/sh
read request
case "$1" in
list)
  echo '{{"store": {{"token": "t1"}}}}'
  if [ "$2" = "gone" ]; then echo "not found" >&2; exit 1; fi
  if [ "$2" = "sub" ]; then echo '{{"books": [{{"id": "c", "title": "Book C", "file_name": "sub/c.zip"}}], "parent": ""}}'; exit 0; fi
  echo '{{"books": [{{"id": "a", "title": "Book A", "file_name": "a.zip"}}, {{"id": "b", "title": "Book B", "file_name": "b.zip"}}, {{"id": "sub", "title": "Sub", "file_name": "sub", "folder": true}}]}}';;
download) cp "{}" "$3"; echo '{{"progress": 1, "total": 1}}';;
esac
"#,
                root.join("source.zip").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let ctx = egui::Context::default();
        let mut reader = MangaReader::with_settings(
            AppSettings {
                page_view_options: PageViewOptions::Single,
                enable_animated_images: false,
                resize_method: ResizeMethod::None,
                library_dir: root.join("library").to_string_lossy().into_owned(),
                ..AppSettings::default()
            },
            None,
        );
        reader.plugins = super::plugins::PluginHost::with(
            vec![PluginManifest {
                id: "test".to_owned(),
                name: "Test".to_owned(),
                command: vec!["./run.sh".to_owned()],
                credentials: Vec::new(),
                auth: false,
                kind: crate::engine::plugin::PluginKind::Books,
                ocr_batch: None,
                dir: root.clone(),
            }],
            crate::engine::credentials::Credentials::memory_only(),
            root.join("cache"),
        );
        let wait = |reader: &mut MangaReader, done: &dyn Fn(&MangaReader) -> bool| {
            for _ in 0..200 {
                reader.poll_plugins(&ctx);
                if done(reader) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("プラグインの答えが来ない");
        };

        // プラグインのタブに移ると、一覧を取りに行き、届いたら並ぶ
        reader.toggle_book_list(ShelfSource::History);
        reader.switch_shelf_tab(ShelfTab::Plugin(0));
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Plugin("test".to_owned(), String::new()))
        );
        wait(&mut reader, &|r| {
            r.thumbnails
                .as_ref()
                .is_some_and(|v| v.cover_paths().len() == 3)
        });
        // プラグインが返した値はログイン情報として保存する
        assert_eq!(
            reader.plugins.credential("test", "token").as_deref(),
            Some("t1")
        );

        // フォルダを選ぶと中に入り、上のフォルダへで戻ると来たフォルダが選ばれている
        reader.choose_thumbnail(2, &ctx);
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Plugin("test".to_owned(), "sub".to_owned()))
        );
        wait(&mut reader, &|r| {
            r.thumbnails
                .as_ref()
                .is_some_and(|v| v.cover_paths().len() == 1)
        });
        assert_eq!(
            reader.thumbnails.as_ref().unwrap().cover_paths()[0],
            root.join("library").join("test").join("sub").join("c.zip")
        );
        // 次にタブを開くときは、このフォルダから
        assert_eq!(reader.plugin_start_folder("test"), "sub");
        reader.go_to_upper_folder();
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Plugin("test".to_owned(), String::new()))
        );
        assert_eq!(reader.thumbnails.as_ref().unwrap().selected(), 2);

        // 覚えていたフォルダが読めなければ（消えたフォルダなど）、一番上に戻れ、次は一番上から始める
        reader
            .config
            .plugin_folders
            .insert("test".to_owned(), "gone".to_owned());
        reader.thumbnails = None;
        reader.toggle_book_list(ShelfSource::Plugin("test".to_owned(), "gone".to_owned()));
        wait(&mut reader, &|r| {
            r.plugins
                .shelves
                .get(&("test".to_owned(), "gone".to_owned()))
                .is_some_and(|shelf| shelf.error.is_some())
        });
        assert_eq!(reader.plugin_start_folder("test"), "");
        reader.go_to_upper_folder();
        assert_eq!(
            reader.shelf_source(),
            Some(ShelfSource::Plugin("test".to_owned(), String::new()))
        );
        let book_b = root.join("library").join("test").join("b.zip");
        assert_eq!(reader.thumbnails.as_ref().unwrap().cover_paths()[1], book_b);
        // 一覧は手元にためる
        assert!(
            root.join("cache")
                .join("plugins")
                .join("test.json")
                .is_file()
        );

        // まだ無い本を選ぶとダウンロードし、終わったら開く
        reader.choose_thumbnail(1, &ctx);
        assert!(reader.thumbnails.is_some());
        wait(&mut reader, &|r| r.zip_path.is_some());
        assert_eq!(reader.zip_path, Some(book_b.clone()));
        assert!(reader.thumbnails.is_none());

        // ダウンロードしてある本は、選ぶとそのまま開く
        reader.switch_shelf_tab(ShelfTab::Plugin(0));
        reader.toggle_book_list(ShelfSource::Plugin("test".to_owned(), String::new()));
        reader.choose_thumbnail(1, &ctx);
        assert!(reader.thumbnails.is_none());
        assert_eq!(reader.zip_path, Some(book_b));

        fs::remove_dir_all(root.join("library")).unwrap();
        fs::remove_dir_all(root.join("cache")).unwrap();
        for name in ["source.zip", "run.sh"] {
            fs::remove_file(root.join(name)).unwrap();
        }
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

    /// タイトルバーは、マンガミーヤと同じ形（ページ番号/総ページ数 : 左から並べたファイル名  Scale  <本>）
    #[test]
    fn the_title_bar_shows_pages_files_scale_and_book_like_mangameeya() {
        let pages = TestPages::new(&[[100, 200], [100, 200], [100, 200]]);
        let ctx = egui::Context::default();
        let draw = |reader: &mut MangaReader| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                let mut action = MangaAction::None;
                reader.render_page_view(ctx, &mut action);
            });
            reader.window_title(&ctx)
        };
        let book = pages.0.file_name().unwrap().to_string_lossy().into_owned();
        let settings = |mode| AppSettings {
            page_view_options: mode,
            enable_animated_images: false,
            resize_method: ResizeMethod::None,
            image_sizing_mode: crate::config::ImageSizingMode::OriginalSize,
            ..AppSettings::default()
        };

        // 何も開いていなければ、ソフトの名前だけ
        let empty = MangaReader::with_settings(settings(PageViewOptions::Single), None);
        assert_eq!(empty.window_title(&ctx), "MangaReeda");

        let mut single = pages.reader(settings(PageViewOptions::Single), &ctx);
        assert_eq!(
            draw(&mut single),
            format!("1/3 : 000.png  Scale=100%  <{book}> - MangaReeda")
        );

        // 右開きの見開きでは、2枚目が左
        let mut spread = pages.reader(settings(PageViewOptions::DoubleRL), &ctx);
        assert_eq!(
            draw(&mut spread),
            format!("1,2/3 : 001.png | 000.png  Scale=100%  <{book}> - MangaReeda")
        );
        let mut left_to_right = pages.reader(settings(PageViewOptions::DoubleLR), &ctx);
        assert!(draw(&mut left_to_right).starts_with("1,2/3 : 000.png | 001.png  Scale="));
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
        // 区切りは OS によって違う（Windows は "\\"）ので、パスの部品で比べる
        assert!(std::path::Path::new(&asking.image_files[2]).ends_with("vol1/000.png"));

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
            keys.show_history,
            keys.show_favorites,
            keys.toggle_favorite,
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
