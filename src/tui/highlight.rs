//! Syntax highlighting of fenced code blocks.

use std::sync::LazyLock;

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

pub fn theme(dark: bool) -> &'static Theme {
    &THEMES.themes[if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    }]
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
