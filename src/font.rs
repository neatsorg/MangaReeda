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
    use std::collections::BTreeSet;
    use std::path::Path;

    /// ソースの文字列（"…" の中）にある文字。コメントは見ない
    fn string_chars(source: &str, chars: &mut BTreeSet<char>) {
        let mut text = source.chars().peekable();
        let (mut in_string, mut in_comment) = (false, false);
        while let Some(c) = text.next() {
            if in_comment {
                in_comment = c != '\n';
            } else if in_string {
                match c {
                    '\\' => {
                        text.next();
                    }
                    '"' => in_string = false,
                    _ => {
                        chars.insert(c);
                    }
                }
            } else if c == '/' && text.peek() == Some(&'/') {
                in_comment = true;
            } else if c == '\'' {
                // 文字のリテラル（'"' など）は飛ばす
                if text.next() == Some('\\') {
                    text.next();
                }
                if text.peek() == Some(&'\'') {
                    text.next();
                }
            } else if c == '"' {
                in_string = true;
            }
        }
    }

    fn rust_files(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }

    /// 画面に出る文字（文言のファイルの値と、ソースの文字列）は、同梱のフォントか egui の予備のフォントで
    /// 描けるものだけにする（無いと四角になる）。ASCII 以外の文字を全部確かめる
    #[test]
    fn text_shown_on_screen_can_be_drawn() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut chars = BTreeSet::new();
        for language in ["ja", "en"] {
            let path = root.join("localize").join(format!("{language}.json"));
            let texts: std::collections::HashMap<String, String> =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            chars.extend(texts.values().flat_map(|text| text.chars()));
        }
        let mut files = Vec::new();
        rust_files(&root.join("src"), &mut files);
        for file in files {
            string_chars(&std::fs::read_to_string(file).unwrap(), &mut chars);
        }
        chars.retain(|c| !c.is_ascii());
        // 文字を集め損ねていないか（見張りの見張り）
        for c in "×▣⛶📁、［？".chars() {
            assert!(chars.contains(&c), "{c} を集められていない");
        }
        let ctx = egui::Context::default();
        super::setup_custom_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let missing: String = ctx.fonts_mut(|fonts| {
            chars
                .iter()
                .copied()
                .filter(|&c| !fonts.has_glyph(&egui::FontId::proportional(14.0), c))
                .collect()
        });
        assert_eq!(missing, "", "描けない文字");
    }
}
