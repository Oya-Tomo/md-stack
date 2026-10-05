//! Markdown dialect shared by the MCP server and the TUI.
//!
//! Both sides must parse identically so that the n-th math event the TUI sees is the n-th
//! expression the server rendered.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

pub fn parser(markdown: &str) -> Parser<'_> {
    Parser::new_ext(
        markdown,
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_MATH
            | Options::ENABLE_GFM,
    )
}

pub struct MathSource {
    pub tex: String,
    pub display: bool,
    /// 1-based line of the expression in the Markdown source.
    pub line: usize,
}

pub fn math_sources(markdown: &str) -> Vec<MathSource> {
    parser(markdown)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            let (tex, display) = match event {
                Event::InlineMath(tex) => (tex, false),
                Event::DisplayMath(tex) => (tex, true),
                _ => return None,
            };
            Some(MathSource {
                tex: tex.into_string(),
                display,
                line: markdown[..range.start].matches('\n').count() + 1,
            })
        })
        .collect()
}

/// The text of the first top-level heading, or else the first non-empty line.
pub fn default_title(markdown: &str) -> String {
    const MAX_CHARS: usize = 60;
    let mut heading: Option<String> = None;
    for event in parser(markdown) {
        match (&mut heading, event) {
            (None, Event::Start(Tag::Heading { level, .. })) if level <= HeadingLevel::H2 => {
                heading = Some(String::new());
            }
            (Some(text), Event::Text(t) | Event::Code(t) | Event::InlineMath(t)) => {
                text.push_str(&t);
            }
            (Some(_), Event::End(TagEnd::Heading(_))) => break,
            _ => {}
        }
    }
    let title = heading
        .filter(|h| !h.trim().is_empty())
        .or_else(|| {
            markdown
                .lines()
                .map(|l| l.trim().trim_start_matches('#').trim())
                .find(|l| !l.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "(untitled)".to_owned());
    let title = title.trim();
    if title.chars().count() > MAX_CHARS {
        title.chars().take(MAX_CHARS - 1).chain(['…']).collect()
    } else {
        title.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_math_with_lines() {
        let md = "# T\n\nInline $a+b$ here.\n\n$$\nx^2\n$$\n\n```\n$not math$\n```\n";
        let math = math_sources(md);
        assert_eq!(math.len(), 2);
        assert_eq!(
            (math[0].tex.as_str(), math[0].display, math[0].line),
            ("a+b", false, 3)
        );
        assert_eq!(
            (math[1].tex.trim(), math[1].display, math[1].line),
            ("x^2", true, 5)
        );
    }

    #[test]
    fn title_prefers_heading() {
        assert_eq!(default_title("intro\n\n## Hello $x$\n"), "Hello x");
        assert_eq!(default_title("\n  first line\nsecond"), "first line");
        assert_eq!(default_title(""), "(untitled)");
    }
}
