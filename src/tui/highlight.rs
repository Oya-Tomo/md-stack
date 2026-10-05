//! Syntax highlighting of fenced code blocks.

use std::sync::LazyLock;
use std::thread;

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::document;

/// bat's curated syntaxes; syntect's own defaults lack common languages such as TypeScript.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

pub fn theme(dark: bool) -> &'static Theme {
    &THEMES.themes[if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    }]
}

/// Highlights the code blocks of `markdowns` on a background thread.
///
/// syntect compiles a language's rules on its first use, which takes a noticeable fraction of a
/// second for large syntaxes such as TypeScript. The compiled rules are shared, so preparing
/// posts as they arrive keeps opening them fast.
pub fn prepare(markdowns: Vec<String>) {
    thread::spawn(move || {
        for markdown in markdowns {
            for block in document::code_blocks(&markdown) {
                // The theme only colors the result; the compiled rules are the same for all.
                highlight(&block.code, &block.lang, theme(true));
            }
        }
    });
}

/// Highlights `code` line by line. Unknown languages are returned unstyled.
pub fn highlight(code: &str, lang: &str, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let Some(syntax) = SYNTAXES
        .find_syntax_by_token(lang)
        .filter(|_| !lang.is_empty())
    else {
        return code
            .lines()
            .map(|l| vec![Span::raw(l.to_owned())])
            .collect();
    };
    let mut highlighter = HighlightLines::new(syntax, theme);
    LinesWithEndings::from(code)
        .map(|line| match highlighter.highlight_line(line, &SYNTAXES) {
            Ok(regions) => regions
                .into_iter()
                .map(|(style, text)| {
                    let fg = style.foreground;
                    Span::styled(
                        text.trim_end_matches('\n').to_owned(),
                        Style::new().fg(Color::Rgb(fg.r, fg.g, fg.b)),
                    )
                })
                .collect(),
            Err(_) => vec![Span::raw(line.trim_end_matches('\n').to_owned())],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether highlighting gave the code more than one color.
    fn is_colored(code: &str, lang: &str) -> bool {
        let lines = highlight(code, lang, theme(true));
        let mut colors: Vec<_> = lines.iter().flatten().map(|span| span.style.fg).collect();
        colors.dedup();
        colors.len() > 1
    }

    #[test]
    fn highlights_languages_missing_from_syntect_defaults() {
        assert!(is_colored("const n: number = 1;\n", "ts"));
        assert!(is_colored(
            "function f(x)\n    return x + 1\nend\n",
            "julia"
        ));
        assert!(is_colored("[package]\nname = \"md-stack\"\n", "toml"));
    }

    #[test]
    fn unknown_languages_are_plain() {
        assert!(!is_colored("anything at all\n", "no-such-language"));
    }
}
