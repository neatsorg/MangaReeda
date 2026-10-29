// このファイルは RustMangaReader（Lycoris52、Apache License 2.0）のコードを元に改変したものです。

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SourceMode {
    Zip,
    Folder,
    Pdf,
    Rar,
}

#[derive(Debug, PartialEq, Copy, Clone, Serialize, Deserialize)]
pub enum PageViewOptions {
    Single,   // single page
    DoubleRL, // double page from right to left
    DoubleLR, // double page from left to right
    TopDown,  // vertical web manga view
}

#[derive(Debug, PartialEq, Copy, Clone, Serialize, Deserialize)]
pub enum LastPageAction {
    GotoNextFile,
    ToFirstPage,
    Nothing,
}

impl LastPageAction {
    /// リピート状態の切り替え順: 非リピート → リピート → フォルダ移動 → 非リピート…
    pub fn next_in_cycle(self) -> Self {
        match self {
            Self::Nothing => Self::ToFirstPage,
            Self::ToFirstPage => Self::GotoNextFile,
            Self::GotoNextFile => Self::Nothing,
        }
    }
}

/// サブフォルダを含むフォルダを開いたときの扱い。
/// 冊ごとに覚えた選択があれば、そちらを優先する
#[derive(Debug, PartialEq, Eq, Copy, Clone, Default, Serialize, Deserialize)]
pub enum SubfolderMode {
    /// 開くたびに確認する
    #[default]
    Ask,
    /// サブフォルダの画像も読む
    Include,
    /// フォルダの直下の画像だけ読む
    Exclude,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone, Serialize, Deserialize)]
pub enum UiLanguage {
    English,
    Japanese,
}

impl Default for UiLanguage {
    fn default() -> Self {
        Self::Japanese
    }
}

#[derive(Debug, PartialEq, Copy, Clone, Serialize, Deserialize)]
pub enum ResizeMethod {
    None,       // Use original resolution
    Nearest,    // Nearest Neighbor (fastest)
    Triangle,   // Bilinear (Moderate)
    CatmullRom, // Bicubic
    Lanczos3,   // High Quality
}

impl ResizeMethod {
    pub fn to_filter(self) -> Option<image::imageops::FilterType> {
        match self {
            ResizeMethod::None => None,
            ResizeMethod::Nearest => Some(image::imageops::FilterType::Nearest),
            ResizeMethod::Triangle => Some(image::imageops::FilterType::Triangle),
            ResizeMethod::CatmullRom => Some(image::imageops::FilterType::CatmullRom),
            ResizeMethod::Lanczos3 => Some(image::imageops::FilterType::Lanczos3),
        }
    }
}

#[derive(Debug, PartialEq, Copy, Clone, Serialize, Deserialize)]
pub enum ImageSizingMode {
    FitHeight,
    FitWidth,
    FitBoth,
    OriginalSize,
    /// 縦サイズを揃える: 見開きの2枚を同じ高さにして、ウインドウに収める
    MatchHeight,
    /// 縮小はするが拡大はしない: 幅と高さに合わせるが、原寸より大きくしない
    ShrinkOnly,
}

impl ImageSizingMode {
    /// ページを縦方向の中央に置くモードか（高さいっぱいに広げないモード）
    pub fn centers_vertically(self) -> bool {
        matches!(self, Self::FitBoth | Self::MatchHeight | Self::ShrinkOnly)
    }
}

impl Default for ImageSizingMode {
    fn default() -> Self {
        Self::FitHeight
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub struct Shortcut {
    pub key: egui::Key,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Shortcut {
    // Helper to create a new shortcut easily
    fn new(key: egui::Key, ctrl: bool, alt: bool, shift: bool) -> Self {
        Self {
            key,
            ctrl,
            alt,
            shift,
        }
    }

    // Helper to format the name for the UI (e.g., "Ctrl + O")
    pub fn format(&self) -> String {
        let mut parts = vec![];
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        let temp = format!("{:?}", self.key);
        parts.push(&temp);
        parts.join(" + ")
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum MangaAction {
    None,
    SlideImageDown,
    SlideImageUp,
    ToggleAutoScroll,
    ReloadCurrentImage,
    ZoomIn,
    ZoomOut,
    NextPage,
    PrevPage,
    OneNextPage,
    OnePrevPage,
    FirstPage,
    LastPage,
    NextFile,
    PrevFile,
    NextFolder,
    PrevFolder,
    FullScreen,
    ViewMode,
    OpenFile,
    QuitApp,
    CycleLastPageAction,
    ScrollNextPage,
    ScrollPrevPage,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
    ToggleLoupe,
    ToggleAutoPageLayout,
    JumpForward,
    JumpBackward,
    ToggleSplitPages,
    ThumbnailsFitAll,
    ThumbnailsLarge,
    CoverThumbnails,
    UpperFolder,
    ShowHistory,
    ShowFavorites,
    ToggleFavorite,
}

impl MangaAction {
    pub const ALL: [Self; 40] = [
        Self::None,
        Self::SlideImageDown,
        Self::SlideImageUp,
        Self::ToggleAutoScroll,
        Self::ReloadCurrentImage,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::NextPage,
        Self::PrevPage,
        Self::OneNextPage,
        Self::OnePrevPage,
        Self::FirstPage,
        Self::LastPage,
        Self::NextFile,
        Self::PrevFile,
        Self::NextFolder,
        Self::PrevFolder,
        Self::FullScreen,
        Self::ViewMode,
        Self::OpenFile,
        Self::QuitApp,
        Self::CycleLastPageAction,
        Self::ScrollNextPage,
        Self::ScrollPrevPage,
        Self::ScrollUp,
        Self::ScrollDown,
        Self::ScrollLeft,
        Self::ScrollRight,
        Self::ToggleLoupe,
        Self::ToggleAutoPageLayout,
        Self::JumpForward,
        Self::JumpBackward,
        Self::ToggleSplitPages,
        Self::ThumbnailsFitAll,
        Self::ThumbnailsLarge,
        Self::CoverThumbnails,
        Self::UpperFolder,
        Self::ShowHistory,
        Self::ShowFavorites,
        Self::ToggleFavorite,
    ];
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum MouseButton {
    Button1,
    Button2,
    Button3,
    Button4,
    Button5,
}

impl MouseButton {
    pub const ALL: [Self; 5] = [
        Self::Button1,
        Self::Button2,
        Self::Button3,
        Self::Button4,
        Self::Button5,
    ];
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum MouseGesture {
    Unassigned,
    Click(MouseButton),
    DoubleClick(MouseButton),
    LongClick(MouseButton),
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum GamepadButton {
    South,
    East,
    North,
    West,
    LeftTrigger,
    LeftTrigger2,
    RightTrigger,
    RightTrigger2,
    Select,
    Start,
    LeftThumb,
    RightThumb,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl GamepadButton {
    pub const ALL: [Self; 16] = [
        Self::South,
        Self::East,
        Self::North,
        Self::West,
        Self::LeftTrigger,
        Self::LeftTrigger2,
        Self::RightTrigger,
        Self::RightTrigger2,
        Self::Select,
        Self::Start,
        Self::LeftThumb,
        Self::RightThumb,
        Self::DPadUp,
        Self::DPadDown,
        Self::DPadLeft,
        Self::DPadRight,
    ];
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct KeyConfig {
    #[serde(default)]
    pub slide_image_down: Option<Shortcut>,
    #[serde(default)]
    pub slide_image_up: Option<Shortcut>,
    #[serde(default)]
    pub toggle_auto_scroll: Option<Shortcut>,
    #[serde(default)]
    pub reload_current_image: Option<Shortcut>,
    pub next_page: Option<Shortcut>,
    pub prev_page: Option<Shortcut>,
    pub one_next_page: Option<Shortcut>,
    pub one_prev_page: Option<Shortcut>,
    pub first_page: Option<Shortcut>,
    pub last_page: Option<Shortcut>,
    pub next_file: Option<Shortcut>,
    pub prev_file: Option<Shortcut>,
    pub next_folder: Option<Shortcut>,
    pub prev_folder: Option<Shortcut>,
    pub fullscreen: Option<Shortcut>,
    pub view_mode: Option<Shortcut>,
    pub open_file: Option<Shortcut>,
    pub quit_app: Option<Shortcut>,
    #[serde(default = "default_cycle_last_page_action_key")]
    pub cycle_last_page_action: Option<Shortcut>,
    #[serde(default)]
    pub scroll_next_page: Option<Shortcut>,
    #[serde(default)]
    pub scroll_prev_page: Option<Shortcut>,
    #[serde(default)]
    pub scroll_up: Option<Shortcut>,
    #[serde(default)]
    pub scroll_down: Option<Shortcut>,
    #[serde(default)]
    pub scroll_left: Option<Shortcut>,
    #[serde(default)]
    pub scroll_right: Option<Shortcut>,
    #[serde(default)]
    pub toggle_loupe: Option<Shortcut>,
    #[serde(default)]
    pub toggle_auto_page_layout: Option<Shortcut>,
    #[serde(default)]
    pub jump_forward: Option<Shortcut>,
    #[serde(default)]
    pub jump_backward: Option<Shortcut>,
    #[serde(default)]
    pub toggle_split_pages: Option<Shortcut>,
    #[serde(default)]
    pub thumbnails_fit_all: Option<Shortcut>,
    #[serde(default)]
    pub thumbnails_large: Option<Shortcut>,
    #[serde(default)]
    pub cover_thumbnails: Option<Shortcut>,
    #[serde(default)]
    pub upper_folder: Option<Shortcut>,
    #[serde(default)]
    pub show_history: Option<Shortcut>,
    #[serde(default)]
    pub show_favorites: Option<Shortcut>,
    #[serde(default)]
    pub toggle_favorite: Option<Shortcut>,
}

// 既存の設定ファイルにこの項目が無いときもミーヤと同じ F8 を割り当てる
fn default_cycle_last_page_action_key() -> Option<Shortcut> {
    Some(Shortcut::new(egui::Key::F8, false, false, false))
}

impl Default for KeyConfig {
    fn default() -> Self {
        Self {
            slide_image_down: None,
            slide_image_up: None,
            toggle_auto_scroll: None,
            reload_current_image: None,
            next_page: Some(Shortcut::new(egui::Key::ArrowLeft, false, false, false)),
            prev_page: Some(Shortcut::new(egui::Key::ArrowRight, false, false, false)),
            one_next_page: Some(Shortcut::new(egui::Key::ArrowLeft, false, false, true)),
            one_prev_page: Some(Shortcut::new(egui::Key::ArrowRight, false, false, true)),
            first_page: Some(Shortcut::new(egui::Key::Home, false, false, false)),
            last_page: Some(Shortcut::new(egui::Key::End, false, false, false)),
            next_file: Some(Shortcut::new(egui::Key::ArrowDown, false, false, false)),
            prev_file: Some(Shortcut::new(egui::Key::ArrowUp, false, false, false)),
            next_folder: Some(Shortcut::new(egui::Key::ArrowLeft, true, false, false)),
            prev_folder: Some(Shortcut::new(egui::Key::ArrowRight, true, false, false)),
            fullscreen: Some(Shortcut::new(egui::Key::Enter, true, false, false)),
            view_mode: Some(Shortcut::new(egui::Key::Enter, false, false, false)),
            open_file: Some(Shortcut::new(egui::Key::O, false, false, false)),
            quit_app: Some(Shortcut::new(egui::Key::Escape, false, false, false)),
            cycle_last_page_action: default_cycle_last_page_action_key(),
            scroll_next_page: None,
            scroll_prev_page: None,
            scroll_up: None,
            scroll_down: None,
            scroll_left: None,
            scroll_right: None,
            toggle_loupe: None,
            toggle_auto_page_layout: None,
            jump_forward: None,
            jump_backward: None,
            toggle_split_pages: None,
            thumbnails_fit_all: None,
            thumbnails_large: None,
            cover_thumbnails: None,
            upper_folder: None,
            show_history: None,
            show_favorites: None,
            toggle_favorite: None,
        }
    }
}

impl KeyConfig {
    /// マンガミーヤのマンガ用設定に近いキー配置。
    /// 対応する操作がまだ無いキーは割り当てない
    pub fn mangameeya() -> Self {
        Self {
            slide_image_down: None,
            slide_image_up: None,
            toggle_auto_scroll: None,
            reload_current_image: None,
            next_page: Some(Shortcut::new(egui::Key::ArrowRight, false, false, false)),
            prev_page: Some(Shortcut::new(egui::Key::ArrowLeft, false, false, false)),
            one_next_page: Some(Shortcut::new(egui::Key::ArrowDown, false, false, false)),
            one_prev_page: Some(Shortcut::new(egui::Key::ArrowUp, false, false, false)),
            first_page: Some(Shortcut::new(egui::Key::Home, false, false, false)),
            last_page: Some(Shortcut::new(egui::Key::End, false, false, false)),
            next_file: Some(Shortcut::new(egui::Key::ArrowRight, false, false, true)),
            prev_file: Some(Shortcut::new(egui::Key::ArrowLeft, false, false, true)),
            next_folder: None,
            prev_folder: None,
            fullscreen: Some(Shortcut::new(egui::Key::Enter, false, false, false)),
            // ミーヤではテンキー0が「先頭ページ挿入」。いちばん近い「奇数／偶数」切り替えを当てる
            view_mode: Some(Shortcut::new(egui::Key::Num0, false, false, false)),
            open_file: Some(Shortcut::new(egui::Key::O, false, false, false)),
            quit_app: Some(Shortcut::new(egui::Key::Escape, false, false, false)),
            cycle_last_page_action: default_cycle_last_page_action_key(),
            scroll_next_page: None,
            scroll_prev_page: None,
            // Ctrl+方向キーで画面のスクロール
            scroll_up: Some(Shortcut::new(egui::Key::ArrowUp, true, false, false)),
            scroll_down: Some(Shortcut::new(egui::Key::ArrowDown, true, false, false)),
            scroll_left: Some(Shortcut::new(egui::Key::ArrowLeft, true, false, false)),
            scroll_right: Some(Shortcut::new(egui::Key::ArrowRight, true, false, false)),
            toggle_loupe: Some(Shortcut::new(egui::Key::L, false, false, false)),
            toggle_auto_page_layout: Some(Shortcut::new(egui::Key::Space, false, false, false)),
            jump_forward: Some(Shortcut::new(egui::Key::ArrowRight, false, true, false)),
            jump_backward: Some(Shortcut::new(egui::Key::ArrowLeft, false, true, false)),
            // ミーヤでもキーは割り当てられていない（CMD139=0）
            toggle_split_pages: None,
            // A = サムネイル（1画面）、H = サムネイル（8列）
            thumbnails_fit_all: Some(Shortcut::new(egui::Key::A, false, false, false)),
            thumbnails_large: Some(Shortcut::new(egui::Key::H, false, false, false)),
            // Shift+↑ = 上のフォルダへ、Shift+↓ = 表紙サムネイル
            cover_thumbnails: Some(Shortcut::new(egui::Key::ArrowDown, false, false, true)),
            upper_folder: Some(Shortcut::new(egui::Key::ArrowUp, false, false, true)),
            show_history: None,
            show_favorites: None,
            toggle_favorite: None,
        }
    }

    /// 縦スクロール表示用のミーヤ風: 方向キーは縦スクロールのまま、ルーペを L に足す
    pub fn mangameeya_top_down() -> Self {
        Self {
            toggle_loupe: Some(Shortcut::new(egui::Key::L, false, false, false)),
            ..Self::top_down_default()
        }
    }

    pub fn top_down_default() -> Self {
        Self {
            slide_image_down: Some(Shortcut::new(egui::Key::ArrowDown, false, false, false)),
            slide_image_up: Some(Shortcut::new(egui::Key::ArrowUp, false, false, false)),
            toggle_auto_scroll: None,
            reload_current_image: None,
            next_page: Some(Shortcut::new(egui::Key::ArrowLeft, false, false, false)),
            prev_page: Some(Shortcut::new(egui::Key::ArrowRight, false, false, false)),
            one_next_page: None,
            one_prev_page: None,
            first_page: Some(Shortcut::new(egui::Key::Home, false, false, false)),
            last_page: Some(Shortcut::new(egui::Key::End, false, false, false)),
            next_file: Some(Shortcut::new(egui::Key::ArrowLeft, false, false, false)),
            prev_file: Some(Shortcut::new(egui::Key::ArrowRight, false, false, false)),
            next_folder: Some(Shortcut::new(egui::Key::ArrowLeft, true, false, false)),
            prev_folder: Some(Shortcut::new(egui::Key::ArrowRight, true, false, false)),
            fullscreen: Some(Shortcut::new(egui::Key::Enter, true, false, false)),
            view_mode: Some(Shortcut::new(egui::Key::Enter, false, false, false)),
            open_file: Some(Shortcut::new(egui::Key::O, false, false, false)),
            quit_app: Some(Shortcut::new(egui::Key::Escape, false, false, false)),
            cycle_last_page_action: default_cycle_last_page_action_key(),
            scroll_next_page: None,
            scroll_prev_page: None,
            scroll_up: None,
            scroll_down: None,
            scroll_left: None,
            scroll_right: None,
            toggle_loupe: None,
            toggle_auto_page_layout: None,
            jump_forward: None,
            jump_backward: None,
            toggle_split_pages: None,
            thumbnails_fit_all: None,
            thumbnails_large: None,
            cover_thumbnails: None,
            upper_folder: None,
            show_history: None,
            show_favorites: None,
            toggle_favorite: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct MouseConfig {
    pub scroll_up: MangaAction,
    pub scroll_down: MangaAction,
    pub button1_click: MangaAction,
    pub button2_click: MangaAction,
    pub button3_click: MangaAction,
    pub button4_click: MangaAction,
    pub button5_click: MangaAction,
    pub button1_double_click: MangaAction,
    pub button2_double_click: MangaAction,
    pub button3_double_click: MangaAction,
    pub button4_double_click: MangaAction,
    pub button5_double_click: MangaAction,
    pub button1_long_click: MangaAction,
    pub button2_long_click: MangaAction,
    pub button3_long_click: MangaAction,
    pub button4_long_click: MangaAction,
    pub button5_long_click: MangaAction,
    /// 右ボタンを押したまま左クリック
    pub button2_held_button1_click: MangaAction,
    /// 左ボタンを押したまま右クリック
    pub button1_held_button2_click: MangaAction,
    /// 右ボタンを押したままホイール
    pub button2_held_scroll_up: MangaAction,
    pub button2_held_scroll_down: MangaAction,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            scroll_up: MangaAction::PrevPage,
            scroll_down: MangaAction::NextPage,
            button1_click: MangaAction::NextPage,
            button2_click: MangaAction::PrevPage,
            button3_click: MangaAction::ViewMode,
            button4_click: MangaAction::None,
            button5_click: MangaAction::None,
            button1_double_click: MangaAction::FullScreen,
            button2_double_click: MangaAction::None,
            button3_double_click: MangaAction::None,
            button4_double_click: MangaAction::None,
            button5_double_click: MangaAction::None,
            button1_long_click: MangaAction::OpenFile,
            button2_long_click: MangaAction::None,
            button3_long_click: MangaAction::None,
            button4_long_click: MangaAction::None,
            button5_long_click: MangaAction::None,
            button2_held_button1_click: MangaAction::None,
            button1_held_button2_click: MangaAction::None,
            button2_held_scroll_up: MangaAction::None,
            button2_held_scroll_down: MangaAction::None,
        }
    }
}

impl MouseConfig {
    /// マンガミーヤのマンガ用設定に近いマウス操作。
    /// ホイールはスクロール兼ページ送りにする（はみ出していればスクロール、収まっていればページ送り）
    pub fn mangameeya() -> Self {
        Self {
            scroll_up: MangaAction::ScrollPrevPage,
            scroll_down: MangaAction::ScrollNextPage,
            button1_click: MangaAction::NextPage,
            button2_click: MangaAction::PrevPage,
            button3_click: MangaAction::ToggleLoupe,
            button4_click: MangaAction::NextFile,
            button5_click: MangaAction::PrevFile,
            button1_double_click: MangaAction::ToggleAutoScroll,
            button2_double_click: MangaAction::None,
            button3_double_click: MangaAction::None,
            button4_double_click: MangaAction::None,
            button5_double_click: MangaAction::None,
            button1_long_click: MangaAction::None,
            button2_long_click: MangaAction::None,
            button3_long_click: MangaAction::None,
            button4_long_click: MangaAction::None,
            button5_long_click: MangaAction::None,
            button2_held_button1_click: MangaAction::OneNextPage,
            button1_held_button2_click: MangaAction::OnePrevPage,
            button2_held_scroll_up: MangaAction::ZoomIn,
            button2_held_scroll_down: MangaAction::ZoomOut,
        }
    }

    /// `held` を押したまま `clicked` をクリックしたときの操作
    pub fn chord_click_action(&self, held: MouseButton, clicked: MouseButton) -> MangaAction {
        match (held, clicked) {
            (MouseButton::Button2, MouseButton::Button1) => self.button2_held_button1_click,
            (MouseButton::Button1, MouseButton::Button2) => self.button1_held_button2_click,
            _ => MangaAction::None,
        }
    }

    /// `held` を押したままホイールを回したときの操作
    pub fn chord_scroll_action(&self, held: MouseButton, up: bool) -> MangaAction {
        match (held, up) {
            (MouseButton::Button2, true) => self.button2_held_scroll_up,
            (MouseButton::Button2, false) => self.button2_held_scroll_down,
            _ => MangaAction::None,
        }
    }

    /// 縦スクロール表示用のミーヤ風: ホイールは縦スクロールのまま、ほかはミーヤ風と同じ
    pub fn mangameeya_top_down() -> Self {
        Self {
            scroll_up: MangaAction::SlideImageUp,
            scroll_down: MangaAction::SlideImageDown,
            ..Self::mangameeya()
        }
    }

    pub fn top_down_default() -> Self {
        Self {
            scroll_up: MangaAction::SlideImageUp,
            scroll_down: MangaAction::SlideImageDown,
            ..Self::default()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct GamepadConfig {
    pub south: MangaAction,
    pub east: MangaAction,
    pub north: MangaAction,
    pub west: MangaAction,
    pub left_trigger: MangaAction,
    pub left_trigger2: MangaAction,
    pub right_trigger: MangaAction,
    pub right_trigger2: MangaAction,
    pub select: MangaAction,
    pub start: MangaAction,
    pub left_thumb: MangaAction,
    pub right_thumb: MangaAction,
    pub dpad_up: MangaAction,
    pub dpad_down: MangaAction,
    pub dpad_left: MangaAction,
    pub dpad_right: MangaAction,
}

impl Default for GamepadConfig {
    fn default() -> Self {
        Self {
            south: MangaAction::ViewMode,
            east: MangaAction::PrevPage,
            north: MangaAction::None,
            west: MangaAction::NextPage,
            left_trigger: MangaAction::PrevFile,
            left_trigger2: MangaAction::PrevFolder,
            right_trigger: MangaAction::NextFile,
            right_trigger2: MangaAction::NextFolder,
            select: MangaAction::QuitApp,
            start: MangaAction::OpenFile,
            left_thumb: MangaAction::FirstPage,
            right_thumb: MangaAction::LastPage,
            dpad_up: MangaAction::PrevFile,
            dpad_down: MangaAction::NextFile,
            dpad_left: MangaAction::NextPage,
            dpad_right: MangaAction::PrevPage,
        }
    }
}

impl GamepadConfig {
    pub fn top_down_default() -> Self {
        Self {
            dpad_up: MangaAction::SlideImageUp,
            dpad_down: MangaAction::SlideImageDown,
            ..Self::default()
        }
    }
}

// ルーペの既定値
fn default_loupe_radius() -> f32 {
    150.0
}

fn default_loupe_magnification() -> f32 {
    2.0
}

fn default_double_click_threshold_ms() -> u64 {
    160
}

/// マンガミーヤのマンガ用設定のダブルクリック判定時間（ini の `DblClickInterval`）
pub const MANGAMEEYA_DOUBLE_CLICK_THRESHOLD_MS: u64 = 200;

fn default_image_panel_background() -> [u8; 4] {
    [40, 40, 40, 255]
}

fn default_top_down_keys() -> KeyConfig {
    KeyConfig::top_down_default()
}

fn default_top_down_mouse() -> MouseConfig {
    MouseConfig::top_down_default()
}

fn default_top_down_gamepad() -> GamepadConfig {
    GamepadConfig::top_down_default()
}

fn default_top_down_image_slide_speed() -> f32 {
    0.05
}

fn default_top_down_image_drag_speed() -> f32 {
    3.0
}

// 既存の設定ファイルにこの項目が無いときも、ミーヤと同じく下に置く
fn default_toolbar_at_bottom() -> bool {
    true
}

fn default_top_toolbar_scale() -> f32 {
    1.0
}

fn default_enable_animated_images() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub resize_method: ResizeMethod,
    pub page_view_options: PageViewOptions,
    #[serde(default)]
    pub auto_page_layout_rl: bool,
    #[serde(default)]
    pub auto_page_layout_lr: bool,
    /// 自動単ページ表示（左右分割）: 1ページ表示で、横長のページを左右に分けて半分ずつ見せる
    #[serde(default)]
    pub split_landscape_pages: bool,
    /// 左から右に読むか。1ページ表示のときの読む向きで、自動見開きをオフにしても向きを忘れないようにする
    #[serde(default)]
    pub left_to_right: bool,
    #[serde(default)]
    pub image_sizing_mode: ImageSizingMode,
    #[serde(default)]
    pub spread_center_offset: f32,
    /// 奇数ページ開始のとき、1枚目を白紙と並べて見開きで見せる（ミーヤの先頭ページ挿入）。
    /// オフなら1枚目を単独で中央に見せる
    #[serde(default)]
    pub cover_blank_page: bool,
    /// ルーペの半径（ピクセル）と倍率
    #[serde(default = "default_loupe_radius")]
    pub loupe_radius: f32,
    #[serde(default = "default_loupe_magnification")]
    pub loupe_magnification: f32,
    #[serde(default = "default_image_panel_background")]
    pub image_panel_background: [u8; 4],
    #[serde(default)]
    pub language: UiLanguage,
    pub settings_width: f32,
    pub show_settings: bool,
    pub transparency_support: bool,
    pub enable_single_file_caching: bool,
    #[serde(default = "default_enable_animated_images")]
    pub enable_animated_images: bool,
    pub image_delay: u64,
    pub keys: KeyConfig,
    #[serde(default = "default_top_down_keys")]
    pub top_down_keys: KeyConfig,
    #[serde(default)]
    pub mouse: MouseConfig,
    #[serde(default = "default_top_down_mouse")]
    pub top_down_mouse: MouseConfig,
    #[serde(default)]
    pub gamepad: GamepadConfig,
    #[serde(default = "default_top_down_gamepad")]
    pub top_down_gamepad: GamepadConfig,
    #[serde(default = "default_double_click_threshold_ms")]
    pub double_click_threshold_ms: u64,
    #[serde(default = "default_double_click_threshold_ms")]
    pub top_down_double_click_threshold_ms: u64,
    #[serde(default = "default_top_down_image_slide_speed")]
    pub top_down_image_slide_speed: f32,
    #[serde(default = "default_top_down_image_drag_speed")]
    pub top_down_image_drag_speed: f32,
    #[serde(default = "default_top_toolbar_scale")]
    pub top_toolbar_scale: f32,
    pub show_top_bar: bool,
    /// ツールバーを画面の下に置く（ミーヤと同じ）。偽なら上
    #[serde(default = "default_toolbar_at_bottom")]
    pub toolbar_at_bottom: bool,
    /// プラグインからダウンロードした本を置くフォルダ。空なら既定（ホームの MangaReeda）
    #[serde(default)]
    pub library_dir: String,
    /// 横断検索で、本の名前を探すためにたどるフォルダ（ダウンロード先のほかに）
    #[serde(default)]
    pub search_folders: Vec<String>,
    /// プラグインごとに、最後に見ていたフォルダ（次に開いたときにそこから）
    #[serde(default)]
    pub plugin_folders: std::collections::HashMap<String, String>,
    /// 本の中の検索で使う OCR のプラグインの id。空なら最初に見つかったもの
    pub ocr_engine: String,
    pub enable_auto_image_byte_fix: bool,
    pub last_page_action: LastPageAction,
    #[serde(default)]
    pub subfolder_mode: SubfolderMode,
}

impl AppSettings {
    /// キーとマウスの割り当てを、通常表示・縦スクロール表示ともにミーヤ風にする
    pub fn apply_mangameeya_controls(&mut self) {
        self.keys = KeyConfig::mangameeya();
        self.mouse = MouseConfig::mangameeya();
        self.double_click_threshold_ms = MANGAMEEYA_DOUBLE_CLICK_THRESHOLD_MS;
        self.top_down_keys = KeyConfig::mangameeya_top_down();
        self.top_down_mouse = MouseConfig::mangameeya_top_down();
        self.top_down_double_click_threshold_ms = MANGAMEEYA_DOUBLE_CLICK_THRESHOLD_MS;
    }

    /// 右から左に読んでいるか。見開きならその向き、1ページ表示なら自動見開きの向き、
    /// どちらも無ければ覚えている向き
    pub fn reads_right_to_left(&self) -> bool {
        match self.page_view_options {
            PageViewOptions::DoubleLR => false,
            PageViewOptions::DoubleRL => true,
            _ if self.auto_page_layout_rl => true,
            _ if self.auto_page_layout_lr => false,
            _ => !self.left_to_right,
        }
    }

    /// 自動見開きのオン・オフを切り替える。縦スクロール表示では何もせず false を返す
    pub fn toggle_auto_page_layout(&mut self) -> bool {
        if self.page_view_options == PageViewOptions::TopDown {
            return false;
        }
        if self.auto_page_layout_rl || self.auto_page_layout_lr {
            self.set_auto_page_layout(None);
        } else {
            self.set_auto_page_layout(Some(self.reads_right_to_left()));
        }
        true
    }

    /// 自動見開きを `right_to_left` の向きでオンにする。None ならオフにして、
    /// ミーヤと同じく1ページ表示にする（読む向きは覚えておく）
    pub fn set_auto_page_layout(&mut self, right_to_left: Option<bool>) {
        let was_on = self.auto_page_layout_rl || self.auto_page_layout_lr;
        match right_to_left {
            Some(right_to_left) => {
                // ミーヤと同じく、自動単ページ表示は1ページ表示でしか使えない
                self.split_landscape_pages = false;
                self.auto_page_layout_rl = right_to_left;
                self.auto_page_layout_lr = !right_to_left;
                self.left_to_right = !right_to_left;
                self.page_view_options = if right_to_left {
                    PageViewOptions::DoubleRL
                } else {
                    PageViewOptions::DoubleLR
                };
            }
            None if was_on => {
                self.left_to_right = self.auto_page_layout_lr;
                self.auto_page_layout_rl = false;
                self.auto_page_layout_lr = false;
                self.page_view_options = PageViewOptions::Single;
            }
            None => {}
        }
    }

    fn auto_page_layout_applies(&self) -> bool {
        (self.auto_page_layout_rl || self.auto_page_layout_lr)
            && self.page_view_options != PageViewOptions::TopDown
    }

    /// 利用者がページ表示を選んだとき。自動見開きがオンでも、オンのまま今のページを選んだ表示にする。
    /// 次にページをめくると自動見開きがまた表示を決める（ミーヤと同じ）。
    /// 見開きを選んだら、読む向き（自動見開きの向きも）をそれに合わせる。
    /// 自動見開きより選んだ表示を優先する（次のページ送りまで）なら true を返す
    pub fn choose_page_view(&mut self, view: PageViewOptions) -> bool {
        let overrides_auto = self.auto_page_layout_applies() && view != PageViewOptions::TopDown;
        // 1ページ表示にしても、それまでの向きで読み続ける
        self.left_to_right = !self.reads_right_to_left();
        if matches!(view, PageViewOptions::DoubleRL | PageViewOptions::DoubleLR) {
            self.left_to_right = view == PageViewOptions::DoubleLR;
            if overrides_auto {
                self.auto_page_layout_rl = view == PageViewOptions::DoubleRL;
                self.auto_page_layout_lr = view == PageViewOptions::DoubleLR;
            }
        }
        self.page_view_options = view;
        // ミーヤと同じく、1ページ表示以外を選ぶと自動単ページ表示は切れる
        if view != PageViewOptions::Single {
            self.split_landscape_pages = false;
        }
        overrides_auto
    }

    /// 自動単ページ表示（左右分割）を切り替える。ミーヤと同じく、オンにすると
    /// 1ページ表示にする（自動見開きも切る）。縦スクロール表示では何もせず false を返す
    pub fn toggle_split_landscape_pages(&mut self) -> bool {
        if self.page_view_options == PageViewOptions::TopDown {
            return false;
        }
        if self.split_landscape_pages {
            self.split_landscape_pages = false;
            return true;
        }
        if self.auto_page_layout_rl || self.auto_page_layout_lr {
            self.set_auto_page_layout(None);
        } else {
            self.choose_page_view(PageViewOptions::Single);
        }
        self.split_landscape_pages = true;
        true
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            resize_method: ResizeMethod::Triangle,
            page_view_options: PageViewOptions::DoubleRL,
            auto_page_layout_rl: false,
            auto_page_layout_lr: false,
            left_to_right: false,
            split_landscape_pages: false,
            library_dir: String::new(),
            search_folders: Vec::new(),
            plugin_folders: std::collections::HashMap::new(),
            ocr_engine: String::new(),
            image_sizing_mode: ImageSizingMode::FitHeight,
            spread_center_offset: 0.0,
            cover_blank_page: false,
            loupe_radius: default_loupe_radius(),
            loupe_magnification: default_loupe_magnification(),
            image_panel_background: default_image_panel_background(),
            language: UiLanguage::Japanese,
            settings_width: 300.0,
            show_settings: false,
            transparency_support: false,
            enable_single_file_caching: true,
            enable_animated_images: default_enable_animated_images(),
            image_delay: 0,
            keys: KeyConfig::default(),
            top_down_keys: KeyConfig::top_down_default(),
            mouse: MouseConfig::default(),
            top_down_mouse: MouseConfig::top_down_default(),
            gamepad: GamepadConfig::default(),
            top_down_gamepad: GamepadConfig::top_down_default(),
            double_click_threshold_ms: 160,
            top_down_double_click_threshold_ms: 160,
            top_down_image_slide_speed: default_top_down_image_slide_speed(),
            top_down_image_drag_speed: default_top_down_image_drag_speed(),
            top_toolbar_scale: default_top_toolbar_scale(),
            show_top_bar: true,
            toolbar_at_bottom: default_toolbar_at_bottom(),
            enable_auto_image_byte_fix: true,
            last_page_action: LastPageAction::GotoNextFile,
            subfolder_mode: SubfolderMode::Ask,
        }
    }
}
