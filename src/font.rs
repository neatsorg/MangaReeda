// このファイルは RustMangaReader（Lycoris52、Apache License 2.0）のコードを元に改変したものです。

use std::sync::Arc;

pub fn setup_custom_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // include the bytes of a .ttf file in your binary:
    fonts.font_data.insert(
        "notosan".to_owned(),
        Arc::from(egui::FontData::from_static(include_bytes!(
            "assets/NotoSansJP-Regular.ttf"
        ))),
    );

    // Put notosan top priority for both Proportional and Monospace
    fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .unwrap()
        .insert(0, "notosan".to_owned());

    fonts
        .families
        .get_mut(&egui::FontFamily::Monospace)
        .unwrap()
        .push("notosan".to_owned());

    // 3. Tell egui to use these fonts
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    /// 画面に出す記号は、同梱のフォントか egui の予備のフォントで描けるものだけにする
    /// （無いと四角になる。記号を足したら、ここにも足す）
    #[test]
    fn symbols_used_on_screen_can_be_drawn() {
        let ctx = egui::Context::default();
        super::setup_custom_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let symbols = "×—…←↑→↓↻⏭⏮▶◀●☁⚙⛶▣✓⬅⬆「」『』〜﹃﹄📁📂📖📚📦🕘🖼";
        let missing: String = ctx.fonts_mut(|fonts| {
            symbols
                .chars()
                .filter(|&c| !fonts.has_glyph(&egui::FontId::proportional(14.0), c))
                .collect()
        });
        assert_eq!(missing, "", "描けない記号");
    }
}
