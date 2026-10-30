//! メニューバー。ミーヤと同じく画面上部に並べ、すべての項目にニーモニックを付ける。
//! Alt+文字（または F10）で開き、開いているあいだは方向キーで移動、文字キーで選択、
//! Enter で決定、Esc で閉じる。egui のメニューにはこの操作が無いので、描画も自前で行う

use super::MangaReader;
use super::thumbnails::ShelfSource;
use crate::config::{
    ImageSizingMode, LastPageAction, MangaAction, PageViewOptions, ResizeMethod, SubfolderMode,
    UiLanguage,
};
use crate::localize::tr;
use eframe::egui;
use std::path::PathBuf;

/// メニューに並べる最近開いた冊の数
const RECENT_BOOKS: usize = 9;
const ITEM_HEIGHT: f32 = 24.0;
const MENU_MIN_WIDTH: f32 = 240.0;
/// チェック・ラジオの印を描く幅
const MARK_WIDTH: f32 = 22.0;

/// メニューの項目を選んだときにすること
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MenuCommand {
    Action(MangaAction),
    OpenFolder,
    OpenRecent(PathBuf),
    PageView(PageViewOptions),
    ReadingDirection {
        right_to_left: bool,
    },
    CoverBlank,
    ResizeMethod(ResizeMethod),
    Sizing(ImageSizingMode),
    Repeat(LastPageAction),
    Subfolder(SubfolderMode),
    Language(UiLanguage),
    ToggleToolbar,
    MangaMeeyaControls,
    Settings,
    GoToPage,
    SearchInBook,
    ChooseCover,
    ImageInfo,
    About,
    /// OCR で本の文字を読み直す（確かめてから）
    ReadWithOcr,
    /// 表紙サムネイルで、一つ前に並べていたものに戻る
    ShelfBack,
    /// すべての本から検索（横断検索）
    CrossSearch,
}

/// 項目の左に付ける印
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

#[derive(Clone, Debug)]
pub(super) enum MenuEntry {
    Item {
        label: String,
        mnemonic: Option<char>,
        command: MenuCommand,
        mark: Mark,
        enabled: bool,
    },
    Submenu {
        label: String,
        mnemonic: Option<char>,
        entries: Vec<MenuEntry>,
    },
    Separator,
}

pub(super) struct Menu {
    label: String,
    mnemonic: char,
    entries: Vec<MenuEntry>,
}

/// 開いているメニューと、選んでいる項目
#[derive(Default, Debug, PartialEq)]
pub(super) struct MenuState {
    open: Option<usize>,
    highlighted: Option<usize>,
    /// 開いているサブメニュー（メニューの中の項目の番号）
    submenu: Option<usize>,
    sub_highlighted: Option<usize>,
    /// 上下キーがサブメニューの中を動くか
    in_submenu: bool,
}

impl MenuState {
    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn open_menu(&mut self, index: usize, entries: &[MenuEntry]) {
        *self = MenuState {
            open: Some(index),
            highlighted: step(entries, None, true),
            ..MenuState::default()
        };
    }
}

fn item(label: &str, mnemonic: char, command: MenuCommand) -> MenuEntry {
    MenuEntry::Item {
        label: label.to_owned(),
        mnemonic: Some(mnemonic),
        command,
        mark: Mark::None,
        enabled: true,
    }
}

fn check(label: &str, mnemonic: char, command: MenuCommand, checked: bool) -> MenuEntry {
    MenuEntry::Item {
        label: label.to_owned(),
        mnemonic: Some(mnemonic),
        command,
        mark: Mark::Check(checked),
        enabled: true,
    }
}

fn radio(label: &str, mnemonic: char, command: MenuCommand, selected: bool) -> MenuEntry {
    MenuEntry::Item {
        label: label.to_owned(),
        mnemonic: Some(mnemonic),
        command,
        mark: Mark::Radio(selected),
        enabled: true,
    }
}

fn submenu(label: &str, mnemonic: char, entries: Vec<MenuEntry>) -> MenuEntry {
    MenuEntry::Submenu {
        label: label.to_owned(),
        mnemonic: Some(mnemonic),
        entries,
    }
}

/// 冊を開いていないときは使えない項目にする
fn needs_book(mut entry: MenuEntry, has_book: bool) -> MenuEntry {
    if let MenuEntry::Item { enabled, .. } = &mut entry {
        *enabled = has_book;
    }
    entry
}

/// 選べる項目か（区切りと、使えない項目は飛ばす）
fn selectable(entry: &MenuEntry) -> bool {
    match entry {
        MenuEntry::Item { enabled, .. } => *enabled,
        MenuEntry::Submenu { .. } => true,
        MenuEntry::Separator => false,
    }
}

/// 上下キーで動いたあとの項目。端まで来たら反対の端へ回る
pub(super) fn step(entries: &[MenuEntry], from: Option<usize>, down: bool) -> Option<usize> {
    let count = entries.len();
    if count == 0 {
        return None;
    }
    let mut index = match (from, down) {
        (None, true) => count - 1,
        (None, false) => 0,
        (Some(index), _) => index,
    };
    for _ in 0..count {
        index = if down {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        if selectable(&entries[index]) {
            return Some(index);
        }
    }
    None
}

fn entry_mnemonic(entry: &MenuEntry) -> Option<char> {
    match entry {
        MenuEntry::Item { mnemonic, .. } | MenuEntry::Submenu { mnemonic, .. } => *mnemonic,
        MenuEntry::Separator => None,
    }
}

/// 文字キーに当たる項目
pub(super) fn find_mnemonic(entries: &[MenuEntry], key: char) -> Option<usize> {
    let key = key.to_ascii_uppercase();
    entries
        .iter()
        .position(|entry| selectable(entry) && entry_mnemonic(entry) == Some(key))
}

/// 「開く...」に (O) を付けて「開く(O)...」にする
fn display_label(label: &str, mnemonic: Option<char>) -> String {
    let Some(mnemonic) = mnemonic else {
        return label.to_owned();
    };
    match label.strip_suffix("...") {
        Some(base) => format!("{base}({mnemonic})..."),
        None => format!("{label}({mnemonic})"),
    }
}

/// 文字・数字のキーを、ニーモニックの文字にする
fn key_char(key: egui::Key) -> Option<char> {
    let name = key.name();
    let mut chars = name.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_ascii_alphanumeric()).then(|| c.to_ascii_uppercase())
}

/// 開いているメニューのダイアログ
pub(super) enum MenuDialog {
    GoToPage {
        text: String,
        focused: bool,
    },
    ImageInfo,
    About,
    /// OCR で読み直す前の確かめ
    ConfirmOcr,
}

impl MangaReader {
    /// 今の状態に合わせたメニューの中身
    pub(super) fn build_menus(&self) -> Vec<Menu> {
        let has_book = self.zip_path.is_some() && !self.image_files.is_empty();
        let book = |entry| needs_book(entry, has_book);
        let config = &self.config;
        let view = config.page_view_options;
        let auto_on = config.auto_page_layout_rl || config.auto_page_layout_lr;

        let mut file = vec![
            item(
                tr("menu.file.open"),
                'O',
                MenuCommand::Action(MangaAction::OpenFile),
            ),
            item(tr("menu.file.open_folder"), 'D', MenuCommand::OpenFolder),
            needs_book(
                item(
                    tr("menu.file.close"),
                    'C',
                    MenuCommand::Action(MangaAction::CloseBook),
                ),
                self.zip_path.is_some(),
            ),
            MenuEntry::Separator,
        ];
        let recent: Vec<_> = self.history.entries.iter().take(RECENT_BOOKS).collect();
        if recent.is_empty() {
            file.push(MenuEntry::Item {
                label: tr("menu.file.no_history").to_owned(),
                mnemonic: None,
                command: MenuCommand::Action(MangaAction::None),
                mark: Mark::None,
                enabled: false,
            });
        }
        for (index, entry) in recent.into_iter().enumerate() {
            let number = char::from_digit(index as u32 + 1, 10).unwrap_or('9');
            file.push(MenuEntry::Item {
                label: format!("{number}: {}", entry.path.to_string_lossy()),
                mnemonic: Some(number),
                command: MenuCommand::OpenRecent(entry.path.clone()),
                mark: Mark::None,
                enabled: true,
            });
        }
        file.extend([
            MenuEntry::Separator,
            item(
                tr("menu.file.quit"),
                'X',
                MenuCommand::Action(MangaAction::QuitApp),
            ),
        ]);

        let right_to_left = config.reads_right_to_left();
        let double = if right_to_left {
            PageViewOptions::DoubleRL
        } else {
            PageViewOptions::DoubleLR
        };
        let view_menu = vec![
            radio(
                tr("menu.view.single"),
                '1',
                MenuCommand::PageView(PageViewOptions::Single),
                view == PageViewOptions::Single,
            ),
            radio(
                tr("menu.view.double"),
                '2',
                MenuCommand::PageView(double),
                matches!(view, PageViewOptions::DoubleRL | PageViewOptions::DoubleLR),
            ),
            radio(
                tr("menu.view.top_down"),
                'W',
                MenuCommand::PageView(PageViewOptions::TopDown),
                view == PageViewOptions::TopDown,
            ),
            submenu(
                tr("menu.view.direction"),
                'D',
                vec![
                    radio(
                        tr("menu.view.rtl"),
                        'R',
                        MenuCommand::ReadingDirection {
                            right_to_left: true,
                        },
                        right_to_left,
                    ),
                    radio(
                        tr("menu.view.ltr"),
                        'L',
                        MenuCommand::ReadingDirection {
                            right_to_left: false,
                        },
                        !right_to_left,
                    ),
                ],
            ),
            MenuEntry::Separator,
            check(
                tr("menu.view.auto_layout"),
                'A',
                MenuCommand::Action(MangaAction::ToggleAutoPageLayout),
                auto_on,
            ),
            check(
                tr("menu.view.split"),
                'S',
                MenuCommand::Action(MangaAction::ToggleSplitPages),
                config.split_landscape_pages,
            ),
            check(
                tr("menu.view.cover_blank"),
                'B',
                MenuCommand::CoverBlank,
                self.is_shifted && config.cover_blank_page,
            ),
            MenuEntry::Separator,
            book(item(
                tr("menu.view.thumbnails"),
                'T',
                MenuCommand::Action(MangaAction::ThumbnailsFitAll),
            )),
            book(item(
                tr("menu.view.thumbnails_large"),
                'G',
                MenuCommand::Action(MangaAction::ThumbnailsLarge),
            )),
            item(
                tr("menu.view.covers"),
                'C',
                MenuCommand::Action(MangaAction::CoverThumbnails),
            ),
            MenuEntry::Separator,
            check(
                tr("menu.view.fullscreen"),
                'F',
                MenuCommand::Action(MangaAction::FullScreen),
                self.is_fullscreen,
            ),
            check(
                tr("menu.view.loupe"),
                'P',
                MenuCommand::Action(MangaAction::ToggleLoupe),
                self.loupe_enabled,
            ),
            check(
                tr("menu.view.toolbar"),
                'O',
                MenuCommand::ToggleToolbar,
                config.show_top_bar,
            ),
        ];

        let sizing = config.image_sizing_mode;
        let resize = config.resize_method;
        let scale_menu = vec![
            radio(
                tr("menu.scale.fit_height"),
                'H',
                MenuCommand::Sizing(ImageSizingMode::FitHeight),
                sizing == ImageSizingMode::FitHeight,
            ),
            radio(
                tr("menu.scale.fit_width"),
                'W',
                MenuCommand::Sizing(ImageSizingMode::FitWidth),
                sizing == ImageSizingMode::FitWidth,
            ),
            radio(
                tr("menu.scale.fit_both"),
                'B',
                MenuCommand::Sizing(ImageSizingMode::FitBoth),
                sizing == ImageSizingMode::FitBoth,
            ),
            radio(
                tr("menu.scale.match_height"),
                'M',
                MenuCommand::Sizing(ImageSizingMode::MatchHeight),
                sizing == ImageSizingMode::MatchHeight,
            ),
            radio(
                tr("menu.scale.shrink_only"),
                'S',
                MenuCommand::Sizing(ImageSizingMode::ShrinkOnly),
                sizing == ImageSizingMode::ShrinkOnly,
            ),
            radio(
                tr("menu.scale.original"),
                'O',
                MenuCommand::Sizing(ImageSizingMode::OriginalSize),
                sizing == ImageSizingMode::OriginalSize,
            ),
            MenuEntry::Separator,
            book(item(
                tr("menu.scale.zoom_in"),
                'I',
                MenuCommand::Action(MangaAction::ZoomIn),
            )),
            book(item(
                tr("menu.scale.zoom_out"),
                'U',
                MenuCommand::Action(MangaAction::ZoomOut),
            )),
            MenuEntry::Separator,
            submenu(
                tr("menu.scale.filter"),
                'F',
                [
                    (ResizeMethod::None, "menu.scale.filter.none", 'O'),
                    (ResizeMethod::Nearest, "menu.scale.filter.nearest", 'N'),
                    (ResizeMethod::Triangle, "menu.scale.filter.linear", 'L'),
                    (
                        ResizeMethod::CatmullRom,
                        "menu.scale.filter.catmullrom",
                        'C',
                    ),
                    (ResizeMethod::Lanczos3, "menu.scale.filter.lanczos3", 'Z'),
                ]
                .into_iter()
                .map(|(method, label, mnemonic)| {
                    radio(
                        tr(label),
                        mnemonic,
                        MenuCommand::ResizeMethod(method),
                        resize == method,
                    )
                })
                .collect(),
            ),
        ];

        let action = |label: &str, mnemonic, action| {
            book(item(label, mnemonic, MenuCommand::Action(action)))
        };
        let repeat = config.last_page_action;
        let move_menu = vec![
            action(tr("menu.move.next"), 'N', MangaAction::NextPage),
            action(tr("menu.move.one_next"), '1', MangaAction::OneNextPage),
            action(tr("menu.move.jump_next"), '2', MangaAction::JumpForward),
            action(tr("menu.move.last"), 'E', MangaAction::LastPage),
            action(tr("menu.move.next_book"), 'B', MangaAction::NextFile),
            action(tr("menu.move.next_folder"), 'F', MangaAction::NextFolder),
            MenuEntry::Separator,
            action(tr("menu.move.prev"), 'P', MangaAction::PrevPage),
            action(tr("menu.move.one_prev"), 'Q', MangaAction::OnePrevPage),
            action(tr("menu.move.jump_prev"), 'W', MangaAction::JumpBackward),
            action(tr("menu.move.first"), 'H', MangaAction::FirstPage),
            action(tr("menu.move.prev_book"), 'V', MangaAction::PrevFile),
            action(tr("menu.move.prev_folder"), 'R', MangaAction::PrevFolder),
            MenuEntry::Separator,
            needs_book(
                item(
                    tr("menu.move.upper"),
                    'U',
                    MenuCommand::Action(MangaAction::UpperFolder),
                ),
                self.can_go_to_upper_folder(),
            ),
            // タブの「戻る」ボタン（Alt+←）と同じ
            needs_book(
                item(tr("menu.move.shelf_back"), 'K', MenuCommand::ShelfBack),
                self.can_go_back_in_shelf(),
            ),
            book(item(tr("menu.move.goto"), 'G', MenuCommand::GoToPage)),
            book(item(tr("menu.move.search"), 'S', MenuCommand::SearchInBook)),
            item(tr("menu.move.cross_search"), 'A', MenuCommand::CrossSearch),
            MenuEntry::Separator,
            submenu(
                tr("menu.move.repeat"),
                'L',
                vec![
                    radio(
                        tr("menu.move.repeat.off"),
                        'N',
                        MenuCommand::Repeat(LastPageAction::Nothing),
                        repeat == LastPageAction::Nothing,
                    ),
                    radio(
                        tr("menu.move.repeat.on"),
                        'R',
                        MenuCommand::Repeat(LastPageAction::ToFirstPage),
                        repeat == LastPageAction::ToFirstPage,
                    ),
                    radio(
                        tr("menu.move.repeat.folder"),
                        'F',
                        MenuCommand::Repeat(LastPageAction::GotoNextFile),
                        repeat == LastPageAction::GotoNextFile,
                    ),
                ],
            ),
        ];

        // 履歴とお気に入りは、表紙サムネイルの画面で並べる
        let shelf = self.shelf_source();
        let favorite = self
            .zip_path
            .as_ref()
            .is_some_and(|book| self.history.is_favorite(book));
        let booklist_menu = vec![
            check(
                tr("menu.booklist.folder"),
                'D',
                MenuCommand::Action(MangaAction::CoverThumbnails),
                matches!(shelf, Some(ShelfSource::Folder(_))),
            ),
            check(
                tr("menu.booklist.history"),
                'H',
                MenuCommand::Action(MangaAction::ShowHistory),
                shelf == Some(ShelfSource::History),
            ),
            check(
                tr("menu.booklist.favorites"),
                'F',
                MenuCommand::Action(MangaAction::ShowFavorites),
                shelf == Some(ShelfSource::Favorites),
            ),
            MenuEntry::Separator,
            book(check(
                tr("menu.booklist.add_favorite"),
                'A',
                MenuCommand::Action(MangaAction::ToggleFavorite),
                favorite,
            )),
            book(item(
                tr("menu.booklist.choose_cover"),
                'C',
                MenuCommand::ChooseCover,
            )),
        ];

        let subfolder = config.subfolder_mode;
        let tools_menu = vec![
            book(check(
                tr("menu.tools.auto_play"),
                'A',
                MenuCommand::Action(MangaAction::ToggleAutoScroll),
                self.auto_scroll_enabled,
            )),
            submenu(
                tr("menu.tools.subfolder"),
                'S',
                [
                    (SubfolderMode::Ask, "settings.subfolder.ask", 'A'),
                    (SubfolderMode::Include, "settings.subfolder.include", 'I'),
                    (SubfolderMode::Exclude, "settings.subfolder.exclude", 'E'),
                ]
                .into_iter()
                .map(|(mode, label, mnemonic)| {
                    radio(
                        tr(label),
                        mnemonic,
                        MenuCommand::Subfolder(mode),
                        subfolder == mode,
                    )
                })
                .collect(),
            ),
            MenuEntry::Separator,
            // 読み直しはまれで時間もかかるので、検索の窓ではなくここに置き、確かめてから始める
            needs_book(
                item(tr("menu.tools.ocr"), 'O', MenuCommand::ReadWithOcr),
                has_book && self.ocr_engine().is_some() && !self.book_ocr_running(),
            ),
            MenuEntry::Separator,
            item(
                tr("menu.tools.mangameeya"),
                'M',
                MenuCommand::MangaMeeyaControls,
            ),
            submenu(
                tr("menu.tools.language"),
                'L',
                vec![
                    radio(
                        "日本語",
                        'J',
                        MenuCommand::Language(UiLanguage::Japanese),
                        config.language == UiLanguage::Japanese,
                    ),
                    radio(
                        "English",
                        'E',
                        MenuCommand::Language(UiLanguage::English),
                        config.language == UiLanguage::English,
                    ),
                ],
            ),
            MenuEntry::Separator,
            item(tr("menu.tools.settings"), 'C', MenuCommand::Settings),
        ];

        let help_menu = vec![
            book(item(
                tr("menu.help.image_info"),
                'I',
                MenuCommand::ImageInfo,
            )),
            item(tr("menu.help.about"), 'V', MenuCommand::About),
        ];

        vec![
            Menu {
                label: tr("menu.file").to_owned(),
                mnemonic: 'F',
                entries: file,
            },
            Menu {
                label: tr("menu.view").to_owned(),
                mnemonic: 'V',
                entries: view_menu,
            },
            Menu {
                label: tr("menu.scale").to_owned(),
                mnemonic: 'S',
                entries: scale_menu,
            },
            Menu {
                label: tr("menu.move").to_owned(),
                mnemonic: 'J',
                entries: move_menu,
            },
            Menu {
                label: tr("menu.booklist").to_owned(),
                mnemonic: 'B',
                entries: booklist_menu,
            },
            Menu {
                label: tr("menu.tools").to_owned(),
                mnemonic: 'T',
                entries: tools_menu,
            },
            Menu {
                label: tr("menu.help").to_owned(),
                mnemonic: 'H',
                entries: help_menu,
            },
        ]
    }

    /// メニューのキー操作。キーを使ったら（ほかの操作に渡さないなら）true
    pub(super) fn handle_menu_keys(&mut self, ctx: &egui::Context) -> Option<MenuCommand> {
        if ctx.wants_keyboard_input() || self.menu_dialog.is_some() {
            return None;
        }
        let presses: Vec<(egui::Key, egui::Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        if presses.is_empty() {
            return None;
        }
        let menus = self.build_menus();
        for (key, modifiers) in presses {
            let command = menu_key(&mut self.menu_state, &menus, key, modifiers);
            if command.is_some() {
                return command;
            }
        }
        None
    }

    /// メニューで選んだことを行う
    pub(super) fn execute_menu_command(&mut self, command: MenuCommand, ctx: &egui::Context) {
        self.menu_state = MenuState::default();
        match command {
            MenuCommand::Action(action) => self.execute_action(action, ctx),
            MenuCommand::OpenFolder => self.open_folder_dialog(),
            MenuCommand::ChooseCover => self.choose_current_page_as_cover(),
            MenuCommand::SearchInBook => self.open_book_search(),
            MenuCommand::OpenRecent(path) => {
                if path.exists() {
                    self.open_source(path, ctx);
                } else {
                    self.show_fading_error(tr("error.history_missing"));
                }
            }
            MenuCommand::PageView(view) => self.set_page_view(view, ctx),
            MenuCommand::ReadingDirection { right_to_left } => {
                self.set_reading_direction(right_to_left, ctx)
            }
            MenuCommand::CoverBlank => {
                // 先頭に白紙を入れる = 白紙と並べる設定で、奇数ページ開始にする
                if self.is_shifted && self.config.cover_blank_page {
                    self.change_shifted_mode(ctx);
                } else {
                    if !self.config.cover_blank_page {
                        self.set_cover_blank_page(true, ctx);
                    }
                    if !self.is_shifted {
                        self.change_shifted_mode(ctx);
                    }
                }
            }
            MenuCommand::ResizeMethod(method) => self.set_resize_method(method, ctx),
            MenuCommand::Sizing(mode) => self.set_image_sizing_mode(mode, ctx),
            MenuCommand::Repeat(action) => self.set_last_page_action(action),
            MenuCommand::Subfolder(mode) => self.set_subfolder_mode(mode),
            MenuCommand::Language(language) => self.set_ui_language(language),
            MenuCommand::ToggleToolbar => {
                self.config.show_top_bar = !self.config.show_top_bar;
                self.save_settings();
            }
            MenuCommand::MangaMeeyaControls => {
                self.config.apply_mangameeya_controls();
                self.save_settings();
                self.show_fading_error(tr("mode.mangameeya_controls"));
            }
            // もう一度選ぶと閉じる（キーボードだけで開け閉めできるように）
            MenuCommand::Settings => self.config.show_settings = !self.config.show_settings,
            MenuCommand::GoToPage => {
                self.menu_dialog = Some(MenuDialog::GoToPage {
                    text: String::new(),
                    focused: false,
                })
            }
            MenuCommand::ImageInfo => self.menu_dialog = Some(MenuDialog::ImageInfo),
            MenuCommand::About => self.menu_dialog = Some(MenuDialog::About),
            MenuCommand::ReadWithOcr => self.menu_dialog = Some(MenuDialog::ConfirmOcr),
            MenuCommand::ShelfBack => self.go_back_in_shelf(),
            MenuCommand::CrossSearch => self.open_cross_search(),
        }
    }

    /// メニューバーと、開いているメニューを描く。項目を選んだらそれを返す
    pub(super) fn render_menu_bar(&mut self, ctx: &egui::Context) -> Option<MenuCommand> {
        let menus = self.build_menus();
        let mut chosen = None;
        let mut title_rects = Vec::with_capacity(menus.len());
        let bar = egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (index, menu) in menus.iter().enumerate() {
                    let open = self.menu_state.open == Some(index);
                    let response = ui.add(
                        egui::Button::new(display_label(&menu.label, Some(menu.mnemonic)))
                            .frame(false)
                            .selected(open),
                    );
                    if response.clicked() {
                        if open {
                            self.menu_state = MenuState::default();
                        } else {
                            self.menu_state.open_menu(index, &menu.entries);
                            // マウスで開いたときは、項目をまだ選ばない
                            self.menu_state.highlighted = None;
                        }
                    } else if response.hovered() && self.menu_state.is_open() && !open {
                        // 開いているあいだは、隣のメニューに触れると切り替える
                        self.menu_state.open_menu(index, &menu.entries);
                        self.menu_state.highlighted = None;
                    }
                    title_rects.push(response.rect);
                }
            });
        });
        let mut inside = vec![bar.response.rect];

        if let Some(open) = self.menu_state.open
            && let (Some(menu), Some(title)) = (menus.get(open), title_rects.get(open))
        {
            let (dropdown, item_rects, clicked) = draw_entries(
                ctx,
                egui::Id::new(("menu", open)),
                title.left_bottom(),
                &menu.entries,
                self.menu_state.highlighted,
            );
            inside.push(dropdown);
            if let Some(hovered) = clicked.hovered {
                if self.menu_state.highlighted != Some(hovered) {
                    self.menu_state.highlighted = Some(hovered);
                    self.menu_state.in_submenu = false;
                    self.menu_state.submenu = None;
                }
                // マウスを乗せたサブメニューは開く
                if matches!(menu.entries[hovered], MenuEntry::Submenu { .. }) {
                    self.menu_state.submenu = Some(hovered);
                }
            }
            if let Some(index) = clicked.clicked {
                match &menu.entries[index] {
                    MenuEntry::Item { command, .. } => chosen = Some(command.clone()),
                    MenuEntry::Submenu { .. } => self.menu_state.submenu = Some(index),
                    MenuEntry::Separator => {}
                }
            }
            if let Some(sub) = self.menu_state.submenu
                && let Some(MenuEntry::Submenu { entries, .. }) = menu.entries.get(sub)
                && let Some(row) = item_rects.get(sub)
            {
                let (rect, _, sub_clicked) = draw_entries(
                    ctx,
                    egui::Id::new(("submenu", open, sub)),
                    row.right_top(),
                    entries,
                    self.menu_state.sub_highlighted,
                );
                inside.push(rect);
                if let Some(hovered) = sub_clicked.hovered {
                    self.menu_state.sub_highlighted = Some(hovered);
                    self.menu_state.in_submenu = true;
                }
                if let Some(index) = sub_clicked.clicked
                    && let MenuEntry::Item { command, .. } = &entries[index]
                {
                    chosen = Some(command.clone());
                }
            }
            // メニューの外をクリックしたら閉じる
            let clicked_outside = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|pos| !inside.iter().any(|rect| rect.contains(pos)))
            });
            if clicked_outside {
                self.menu_state = MenuState::default();
            }
        }
        chosen
    }

    /// メニューから開くダイアログ（指定ページへ・画像情報・バージョン情報）
    pub(super) fn render_menu_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.menu_dialog.as_mut() else {
            return;
        };
        let mut close = false;
        let mut go_to = None;
        let mut start_ocr = false;
        let response = match dialog {
            MenuDialog::GoToPage { text, focused } => {
                let count = self.image_files.len();
                egui::Modal::new(egui::Id::new("go_to_page")).show(ctx, |ui| {
                    ui.heading(tr("dialog.go_to_page.title"));
                    ui.label(format!("[ 1 - {count} ]"));
                    let edit = ui.add(egui::TextEdit::singleline(text).desired_width(160.0));
                    if !*focused {
                        edit.request_focus();
                        *focused = true;
                    }
                    let entered =
                        edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button(tr("dialog.go_to_page.go")).clicked() || entered {
                            match text.trim().parse::<usize>() {
                                Ok(page) if (1..=count).contains(&page) => go_to = Some(page - 1),
                                _ => {}
                            }
                            close = true;
                        }
                        if ui.button(tr("subfolder.cancel")).clicked() {
                            close = true;
                        }
                    });
                })
            }
            MenuDialog::ImageInfo => {
                let rows = self.image_info_rows();
                egui::Modal::new(egui::Id::new("image_info")).show(ctx, |ui| {
                    ui.heading(tr("dialog.image_info.title"));
                    egui::Grid::new("image_info_grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            for (name, value) in rows {
                                ui.label(name);
                                ui.label(value);
                                ui.end_row();
                            }
                        });
                    if ui.button("OK").clicked() {
                        close = true;
                    }
                })
            }
            MenuDialog::About => egui::Modal::new(egui::Id::new("about")).show(ctx, |ui| {
                ui.heading(format!("MangaReeda {}", env!("CARGO_PKG_VERSION")));
                ui.label(tr("dialog.about.description"));
                ui.label(tr("dialog.about.license"));
                if ui.button("OK").clicked() {
                    close = true;
                }
            }),
            MenuDialog::ConfirmOcr => {
                let engine = self
                    .ocr_engine()
                    .map(|engine| super::book_search::short_name(&engine.name))
                    .unwrap_or_default();
                let body = tr("dialog.ocr.body")
                    .replace("{engine}", &engine)
                    .replace("{pages}", &self.image_files.len().to_string());
                egui::Modal::new(egui::Id::new("confirm_ocr")).show(ctx, |ui| {
                    ui.heading(tr("dialog.ocr.title"));
                    ui.label(body);
                    ui.horizontal(|ui| {
                        if ui.button(tr("dialog.ocr.start")).clicked() {
                            start_ocr = true;
                            close = true;
                        }
                        if ui.button(tr("subfolder.cancel")).clicked() {
                            close = true;
                        }
                    });
                })
            }
        };
        if close || response.should_close() {
            self.menu_dialog = None;
        }
        // 進み具合は検索の窓に出す
        if start_ocr {
            self.open_book_search();
            self.start_book_ocr();
        }
        if let Some(index) = go_to {
            self.go_to_page(index, ctx);
        }
    }

    /// 画像情報に出す行: 表示中のページごとに、ファイル名・ページ番号・原寸サイズ、最後に冊
    fn image_info_rows(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        let count = self.image_files.len();
        let shown = if self.textures[1].is_some() && !self.is_cover_blank(&self.textures[1]) {
            2
        } else {
            1
        };
        for index in self.current_index..(self.current_index + shown).min(count) {
            let name = &self.image_files[index];
            let file_name = name.rsplit(['/', '\\']).next().unwrap_or(name);
            rows.push((
                tr("dialog.image_info.file").to_owned(),
                file_name.to_owned(),
            ));
            rows.push((
                tr("dialog.image_info.page").to_owned(),
                format!("{} / {count}", index + 1),
            ));
            if let Some([width, height]) = self.page_dimensions(index) {
                rows.push((
                    tr("dialog.image_info.size").to_owned(),
                    format!("{width} × {height}"),
                ));
            }
        }
        if let Some(book) = &self.zip_path {
            rows.push((
                tr("dialog.image_info.book").to_owned(),
                book.to_string_lossy().into_owned(),
            ));
        }
        rows
    }
}

/// 項目の描画結果: マウスを乗せている項目とクリックした項目
#[derive(Default)]
struct Picked {
    hovered: Option<usize>,
    clicked: Option<usize>,
}

/// メニューの項目を `origin` から下に並べて描く。（全体の範囲, 各項目の範囲, 結果）を返す
fn draw_entries(
    ctx: &egui::Context,
    id: egui::Id,
    origin: egui::Pos2,
    entries: &[MenuEntry],
    highlighted: Option<usize>,
) -> (egui::Rect, Vec<egui::Rect>, Picked) {
    let mut picked = Picked::default();
    let mut rects = vec![egui::Rect::NOTHING; entries.len()];
    let area = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(origin)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(MENU_MIN_WIDTH);
                let font = egui::TextStyle::Button.resolve(ui.style());
                // いちばん長い項目に合わせた幅
                let width = entries
                    .iter()
                    .filter_map(|entry| match entry {
                        MenuEntry::Item {
                            label, mnemonic, ..
                        }
                        | MenuEntry::Submenu {
                            label, mnemonic, ..
                        } => Some(
                            ui.painter()
                                .layout_no_wrap(
                                    display_label(label, *mnemonic),
                                    font.clone(),
                                    egui::Color32::WHITE,
                                )
                                .size()
                                .x,
                        ),
                        MenuEntry::Separator => None,
                    })
                    .fold(MENU_MIN_WIDTH, |max, width| {
                        max.max(width + MARK_WIDTH * 2.5)
                    });
                for (index, entry) in entries.iter().enumerate() {
                    let (label, mnemonic, mark, enabled, is_submenu) = match entry {
                        MenuEntry::Separator => {
                            ui.separator();
                            continue;
                        }
                        MenuEntry::Item {
                            label,
                            mnemonic,
                            mark,
                            enabled,
                            ..
                        } => (label, mnemonic, *mark, *enabled, false),
                        MenuEntry::Submenu {
                            label, mnemonic, ..
                        } => (label, mnemonic, Mark::None, true, true),
                    };
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(width, ITEM_HEIGHT),
                        if enabled {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        },
                    );
                    rects[index] = rect;
                    if enabled && response.hovered() {
                        picked.hovered = Some(index);
                    }
                    if enabled && response.clicked() {
                        picked.clicked = Some(index);
                    }
                    let visuals = ui.visuals();
                    if enabled && (highlighted == Some(index) || response.hovered()) {
                        ui.painter()
                            .rect_filled(rect, 2.0, visuals.selection.bg_fill);
                    }
                    let color = if enabled {
                        visuals.text_color()
                    } else {
                        visuals.weak_text_color()
                    };
                    let mark_text = match mark {
                        Mark::Check(true) => "✓",
                        Mark::Radio(true) => "●",
                        _ => "",
                    };
                    let painter = ui.painter();
                    painter.text(
                        rect.left_center() + egui::vec2(MARK_WIDTH * 0.5, 0.0),
                        egui::Align2::CENTER_CENTER,
                        mark_text,
                        font.clone(),
                        color,
                    );
                    painter.text(
                        rect.left_center() + egui::vec2(MARK_WIDTH, 0.0),
                        egui::Align2::LEFT_CENTER,
                        display_label(label, *mnemonic),
                        font.clone(),
                        color,
                    );
                    if is_submenu {
                        painter.text(
                            rect.right_center() - egui::vec2(MARK_WIDTH * 0.5, 0.0),
                            egui::Align2::CENTER_CENTER,
                            "▶",
                            font.clone(),
                            color,
                        );
                    }
                }
            });
        });
    (area.response.rect, rects, picked)
}

/// メニューのキー操作を1つ処理する。項目を決定したらそのコマンドを返す
pub(super) fn menu_key(
    state: &mut MenuState,
    menus: &[Menu],
    key: egui::Key,
    modifiers: egui::Modifiers,
) -> Option<MenuCommand> {
    let pressed_char = key_char(key);
    let Some(open) = state.open else {
        // 閉じているときは、Alt+文字でそのメニュー、F10 で最初のメニューを開く
        if modifiers.alt
            && !modifiers.ctrl
            && let Some(c) = pressed_char
            && let Some(index) = menus.iter().position(|menu| menu.mnemonic == c)
        {
            state.open_menu(index, &menus[index].entries);
        } else if key == egui::Key::F10 && modifiers.is_none() && !menus.is_empty() {
            state.open_menu(0, &menus[0].entries);
        }
        return None;
    };
    let entries = &menus[open].entries;
    let sub_entries = state.submenu.and_then(|index| match entries.get(index) {
        Some(MenuEntry::Submenu { entries, .. }) => Some(entries.as_slice()),
        _ => None,
    });
    // 開いているときに Alt+文字 なら別のメニューへ
    if modifiers.alt
        && let Some(c) = pressed_char
        && let Some(index) = menus.iter().position(|menu| menu.mnemonic == c)
    {
        state.open_menu(index, &menus[index].entries);
        return None;
    }
    let switch_menu = |state: &mut MenuState, forward: bool| {
        let count = menus.len();
        let index = if forward {
            (open + 1) % count
        } else {
            (open + count - 1) % count
        };
        state.open_menu(index, &menus[index].entries);
    };
    match key {
        egui::Key::Escape => {
            if state.in_submenu || state.submenu.is_some() {
                state.submenu = None;
                state.sub_highlighted = None;
                state.in_submenu = false;
            } else {
                *state = MenuState::default();
            }
        }
        egui::Key::ArrowDown | egui::Key::ArrowUp => {
            let down = key == egui::Key::ArrowDown;
            match (state.in_submenu, sub_entries) {
                (true, Some(sub)) => state.sub_highlighted = step(sub, state.sub_highlighted, down),
                _ => {
                    state.highlighted = step(entries, state.highlighted, down);
                    state.submenu = None;
                    state.in_submenu = false;
                }
            }
        }
        egui::Key::ArrowRight => {
            if state.in_submenu {
                switch_menu(state, true);
            } else if let Some(index) = state.highlighted
                && let Some(MenuEntry::Submenu { entries: sub, .. }) = entries.get(index)
            {
                state.submenu = Some(index);
                state.in_submenu = true;
                state.sub_highlighted = step(sub, None, true);
            } else {
                switch_menu(state, true);
            }
        }
        egui::Key::ArrowLeft => {
            if state.in_submenu {
                state.submenu = None;
                state.sub_highlighted = None;
                state.in_submenu = false;
            } else {
                switch_menu(state, false);
            }
        }
        egui::Key::Enter | egui::Key::Space => {
            return match (state.in_submenu, sub_entries) {
                (true, Some(sub)) => activate(state, sub, state.sub_highlighted, true),
                _ => activate(state, entries, state.highlighted, false),
            };
        }
        _ => {
            let c = pressed_char?;
            return match (state.in_submenu, sub_entries) {
                (true, Some(sub)) => {
                    let index = find_mnemonic(sub, c);
                    activate(state, sub, index, true)
                }
                _ => {
                    let index = find_mnemonic(entries, c);
                    activate(state, entries, index, false)
                }
            };
        }
    }
    None
}

/// 項目を決定する。サブメニューなら開き、項目ならメニューを閉じてコマンドを返す
fn activate(
    state: &mut MenuState,
    entries: &[MenuEntry],
    index: Option<usize>,
    in_submenu: bool,
) -> Option<MenuCommand> {
    let index = index?;
    match entries.get(index)? {
        MenuEntry::Item {
            command,
            enabled: true,
            ..
        } => {
            *state = MenuState::default();
            Some(command.clone())
        }
        MenuEntry::Submenu { entries: sub, .. } if !in_submenu => {
            state.highlighted = Some(index);
            state.submenu = Some(index);
            state.in_submenu = true;
            state.sub_highlighted = step(sub, None, true);
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menus() -> Vec<Menu> {
        vec![
            Menu {
                label: "File".into(),
                mnemonic: 'F',
                entries: vec![
                    item("Open...", 'O', MenuCommand::Action(MangaAction::OpenFile)),
                    MenuEntry::Item {
                        label: "Close".into(),
                        mnemonic: Some('C'),
                        command: MenuCommand::Action(MangaAction::CloseBook),
                        mark: Mark::None,
                        enabled: false,
                    },
                    MenuEntry::Separator,
                    item("Quit", 'X', MenuCommand::Action(MangaAction::QuitApp)),
                ],
            },
            Menu {
                label: "View".into(),
                mnemonic: 'V',
                entries: vec![
                    item(
                        "Single",
                        '1',
                        MenuCommand::PageView(PageViewOptions::Single),
                    ),
                    submenu(
                        "Direction",
                        'D',
                        vec![
                            item(
                                "Right to left",
                                'R',
                                MenuCommand::ReadingDirection {
                                    right_to_left: true,
                                },
                            ),
                            item(
                                "Left to right",
                                'L',
                                MenuCommand::ReadingDirection {
                                    right_to_left: false,
                                },
                            ),
                        ],
                    ),
                ],
            },
        ]
    }

    fn press(state: &mut MenuState, menus: &[Menu], key: egui::Key) -> Option<MenuCommand> {
        menu_key(state, menus, key, egui::Modifiers::NONE)
    }

    /// 同じ段にニーモニックの重なりが無いか（重なると文字キーで選べない項目ができる）
    fn assert_unique_mnemonics(entries: &[MenuEntry], where_: &str) {
        let mut seen = Vec::new();
        for entry in entries {
            if let Some(c) = entry_mnemonic(entry) {
                assert!(!seen.contains(&c), "{where_} で {c} が重なっている");
                seen.push(c);
            }
            if let MenuEntry::Submenu { entries, label, .. } = entry {
                assert_unique_mnemonics(entries, label);
            }
        }
    }

    #[test]
    fn every_menu_level_has_unique_mnemonics() {
        let reader = MangaReader::with_settings(crate::config::AppSettings::default(), None);
        let menus = reader.build_menus();
        let titles: Vec<char> = menus.iter().map(|menu| menu.mnemonic).collect();
        for (index, c) in titles.iter().enumerate() {
            assert!(
                !titles[index + 1..].contains(c),
                "メニューの {c} が重なっている"
            );
        }
        for menu in &menus {
            assert_unique_mnemonics(&menu.entries, &menu.label);
        }
    }

    #[test]
    fn labels_carry_their_mnemonic() {
        assert_eq!(display_label("開く...", Some('O')), "開く(O)...");
        assert_eq!(display_label("閉じる", Some('C')), "閉じる(C)");
        assert_eq!(display_label("1: book.zip", None), "1: book.zip");
    }

    #[test]
    fn steps_skip_separators_and_disabled_items_and_wrap() {
        let entries = &menus()[0].entries;
        assert_eq!(step(entries, None, true), Some(0));
        // 使えない「Close」と区切りを飛ばす
        assert_eq!(step(entries, Some(0), true), Some(3));
        assert_eq!(step(entries, Some(3), true), Some(0));
        assert_eq!(step(entries, Some(0), false), Some(3));
        assert_eq!(find_mnemonic(entries, 'x'), Some(3));
        assert_eq!(find_mnemonic(entries, 'C'), None);
    }

    #[test]
    fn alt_and_letters_open_menus_and_choose_items() {
        let menus = menus();
        let mut state = MenuState::default();
        // Alt なしの文字では開かない
        assert_eq!(press(&mut state, &menus, egui::Key::F), None);
        assert!(!state.is_open());
        menu_key(&mut state, &menus, egui::Key::F, egui::Modifiers::ALT);
        assert_eq!((state.open, state.highlighted), (Some(0), Some(0)));
        // 文字キーで決定してメニューを閉じる
        assert_eq!(
            press(&mut state, &menus, egui::Key::X),
            Some(MenuCommand::Action(MangaAction::QuitApp))
        );
        assert!(!state.is_open());

        // F10 で最初のメニュー、右で隣のメニューへ
        press(&mut state, &menus, egui::Key::F10);
        assert_eq!(state.open, Some(0));
        press(&mut state, &menus, egui::Key::ArrowRight);
        assert_eq!(state.open, Some(1));
        // 下で「Direction」、右でサブメニューを開き、下と Enter で決定
        press(&mut state, &menus, egui::Key::ArrowDown);
        press(&mut state, &menus, egui::Key::ArrowRight);
        assert!(state.in_submenu);
        assert_eq!(state.sub_highlighted, Some(0));
        press(&mut state, &menus, egui::Key::ArrowDown);
        assert_eq!(
            press(&mut state, &menus, egui::Key::Enter),
            Some(MenuCommand::ReadingDirection {
                right_to_left: false
            })
        );

        // サブメニューの文字キーと、Esc で1段ずつ閉じる
        menu_key(&mut state, &menus, egui::Key::V, egui::Modifiers::ALT);
        press(&mut state, &menus, egui::Key::D);
        assert!(state.in_submenu);
        press(&mut state, &menus, egui::Key::Escape);
        assert!(state.is_open() && !state.in_submenu);
        press(&mut state, &menus, egui::Key::Escape);
        assert!(!state.is_open());
    }
}
