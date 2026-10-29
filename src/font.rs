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

    /// ソースの文字列と文字のリテラルにある文字（書き方によらず、エスケープも戻した中身）。
    /// 字句解析で読むので、コメントは入らない。画面に出ないものは飛ばす:
    /// 属性（説明のコメント `///` は `#[doc = "…"]` になる）、テストのコード（`#[cfg(test)]` の付いたもの）、
    /// 範囲（`'a'..='z'`）の端の文字
    fn string_chars(source: &str, chars: &mut BTreeSet<char>) {
        let tokens: proc_macro2::TokenStream = source.parse().expect("Rust として読めない");
        literal_chars(tokens, chars);
    }

    fn literal_chars(tokens: proc_macro2::TokenStream, chars: &mut BTreeSet<char>) {
        use proc_macro2::{Delimiter, TokenTree};
        let mut tokens = tokens.into_iter().peekable();
        // 直前に続いた記号（範囲の `..` や `..=` を見分ける）
        let mut puncts = String::new();
        while let Some(token) = tokens.next() {
            match &token {
                TokenTree::Punct(punct) if punct.as_char() == '#' => {
                    puncts.clear();
                    if matches!(tokens.peek(), Some(TokenTree::Punct(p)) if p.as_char() == '!') {
                        tokens.next();
                    }
                    let Some(TokenTree::Group(attribute)) = tokens.next() else {
                        continue;
                    };
                    // テストのコードは、次の項目（`{…}` か `;` まで）ごと飛ばす
                    if attribute.stream().to_string().replace(' ', "") == "cfg(test)" {
                        for item in tokens.by_ref() {
                            match item {
                                TokenTree::Group(group)
                                    if group.delimiter() == Delimiter::Brace =>
                                {
                                    break;
                                }
                                TokenTree::Punct(punct) if punct.as_char() == ';' => break,
                                _ => {}
                            }
                        }
                    }
                    continue;
                }
                TokenTree::Punct(punct) => {
                    puncts.push(punct.as_char());
                    continue;
                }
                TokenTree::Group(group) => literal_chars(group.stream(), chars),
                TokenTree::Literal(literal) => match syn::Lit::new(literal.clone()) {
                    syn::Lit::Str(text) => chars.extend(text.value().chars()),
                    syn::Lit::Char(c) => {
                        let range_end = puncts.ends_with("..") || puncts.ends_with("..=");
                        let range_start = matches!(tokens.peek(), Some(TokenTree::Punct(p)) if p.as_char() == '.');
                        if !range_end && !range_start {
                            chars.insert(c.value());
                        }
                    }
                    _ => {}
                },
                TokenTree::Ident(_) => {}
            }
            puncts.clear();
        }
    }

    /// 文字の集め方: エスケープ・生文字列・ブロックコメント・文字のリテラル・文字列の中の // を正しく扱う
    #[test]
    fn literals_are_read_like_rust() {
        let source = r##"
            /// 説明のコメントの ✕ は描かない
            fn f() {
                // 行のコメントの ① も
                /* ブロックの " ② も /* 入れ子の ③ も */ */
                label("\u{1F980}");
                label(r#"引用"④"終わり"#);
                label(r"\⑤");
                label("// ⑥ は文字列の中");
                let quote = '"';
                let mark = '⑦';
                let wide = matches!(c, '⑧'..='⑨');
                let open = ('⑩'..'⑪').contains(&c);
            }
            #[cfg(test)]
            mod tests {
                fn t() { label("⑫"); }
            }
            fn g() { label("⑬"); }
        "##;
        let mut chars = BTreeSet::new();
        string_chars(source, &mut chars);
        // 文字列と文字のリテラルの中身は拾う
        for c in "引用終わり④⑤⑥⑦⑬🦀".chars() {
            assert!(chars.contains(&c), "{c} を拾っていない");
        }
        // コメントの中、範囲の端、テストのコードは拾わない
        for c in "✕①②③⑧⑨⑩⑪⑫".chars() {
            assert!(!chars.contains(&c), "{c} を拾っている");
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
