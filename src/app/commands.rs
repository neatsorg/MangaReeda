//! 設定を変えて表示し直す操作。設定画面とメニューバーの両方から使う

use super::MangaReader;
use crate::config::{
    ImageSizingMode, LastPageAction, PageViewOptions, ResizeMethod, SubfolderMode, UiLanguage,
};
use crate::localize::set_language;
use eframe::egui;

impl MangaReader {
    /// ページ表示（1ページ・見開き・縦スクロール）を選ぶ
    pub(super) fn set_page_view(&mut self, view: PageViewOptions, ctx: &egui::Context) {
        let previous_page_view = self.config.page_view_options;
        self.manual_page_view = self.config.choose_page_view(view);
        self.reset_buffer();
        self.texture_cache.clear();
        self.reset_pan();
        if self.config.page_view_options == PageViewOptions::TopDown {
            self.textures = [None, None];
            self.top_down_textures = vec![None; self.image_files.len()];
            self.top_down_scroll_offset = 0.0;
            self.ensure_top_down_loaded_around(self.current_index, ctx);
        } else {
            if previous_page_view == PageViewOptions::TopDown {
                self.top_down_textures.clear();
            }
            self.load_current_pair(ctx);
        }
        self.save_settings();
    }

    /// 見開きの向きを選ぶ。1ページ表示や縦スクロール表示のときは、向きだけを覚えておく
    pub(super) fn set_reading_direction(&mut self, right_to_left: bool, ctx: &egui::Context) {
        match self.config.page_view_options {
            PageViewOptions::DoubleRL | PageViewOptions::DoubleLR => {
                let view = if right_to_left {
                    PageViewOptions::DoubleRL
                } else {
                    PageViewOptions::DoubleLR
                };
                self.set_page_view(view, ctx);
            }
            _ if self.config.auto_page_layout_rl || self.config.auto_page_layout_lr => {
                self.set_auto_page_layout(Some(right_to_left), ctx);
            }
            _ => {
                self.config.left_to_right = !right_to_left;
                self.save_settings();
            }
        }
    }

    /// 自動見開きを `right_to_left` の向きでオンにする。None ならオフ
    pub(super) fn set_auto_page_layout(&mut self, right_to_left: Option<bool>, ctx: &egui::Context) {
        self.config.set_auto_page_layout(right_to_left);
        self.reset_buffer();
        self.texture_cache.clear();
        self.clear_animated_image_cache();
        self.reset_pan();
        self.load_current_pair(ctx);
        self.save_settings();
    }

    /// 奇数ページ開始のとき、1枚目を白紙と並べるか
    pub(super) fn set_cover_blank_page(&mut self, enabled: bool, ctx: &egui::Context) {
        self.config.cover_blank_page = enabled;
        self.sync_cover_blank(ctx);
        self.save_settings();
    }

    pub(super) fn set_resize_method(&mut self, method: ResizeMethod, ctx: &egui::Context) {
        self.config.resize_method = method;
        self.reset_buffer();
        self.texture_cache.clear();
        self.load_current_pair(ctx);
        self.save_settings();
    }

    pub(super) fn set_image_sizing_mode(&mut self, mode: ImageSizingMode, ctx: &egui::Context) {
        self.config.image_sizing_mode = mode;
        self.reset_buffer();
        self.texture_cache.clear();
        self.reset_pan();
        self.load_current_pair(ctx);
        self.save_settings();
    }

    pub(super) fn set_last_page_action(&mut self, action: LastPageAction) {
        self.config.last_page_action = action;
        self.save_settings();
    }

    pub(super) fn set_subfolder_mode(&mut self, mode: SubfolderMode) {
        self.config.subfolder_mode = mode;
        self.save_settings();
    }

    pub(super) fn set_ui_language(&mut self, language: UiLanguage) {
        self.config.language = language;
        set_language(language);
        self.save_settings();
    }

    /// 冊を閉じて、何も開いていない状態に戻す。読んでいた位置は履歴に残す
    pub(super) fn close_book(&mut self) {
        if self.zip_path.is_none() {
            return;
        }
        self.remember_current_book();
        self.stop_auto_scroll();
        self.forget_thumbnails();
        self.reset_buffer();
        self.texture_cache.clear();
        self.image_dimensions.clear();
        self.clear_animated_image_cache();
        self.clear_animation_scan_state();
        self.clear_active_animations();
        self.clear_pdf_document_cache();
        self.reset_pan();
        self.zip_path = None;
        self.image_files.clear();
        self.all_zips_in_folder.clear();
        self.textures = [None, None];
        self.top_down_textures.clear();
        self.current_index = 0;
        self.manual_page_view = false;
        self.split_second_half_of = None;
        self.zip_name_display = None;
        self.page_indicator_time = None;
    }
}
