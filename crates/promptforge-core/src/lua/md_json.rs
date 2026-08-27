//! `md_to_json`: chunk markdown into a flat, typed block list.
//!
//! A pure function installed as a Lua global in every section VM. No I/O, no
//! observer, no budgets; parsing is infallible (pulldown-cmark accepts any
//! input). A non-string argument fails through mlua's automatic type error.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use serde::Serialize;

use super::{Error, Lua, LuaSerdeExt, Result};

/// One typed block in a flat, document-ordered markdown walk.
///
/// `lang` is present only on `"code_block"` (empty string when the fence has
/// no info string, or for indented code). Every other block omits the field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MdBlock {
    /// Block kind: `"h1"`..`"h6"`, `"paragraph"`, `"code_block"`, `"list"`,
    /// `"table"`, `"blockquote"`, `"html_block"`, or `"thematic_break"`.
    #[serde(rename = "type")]
    block_type: String,
    /// Raw markdown of the block. Headings are concatenated inline title text
    /// (no `#` markers). Code blocks are the fence-stripped, unindented body.
    content: String,
    /// 1-based source line where the block starts.
    line: u32,
    /// Heading path from the document root to this block. Empty before the
    /// first heading. A heading block's path includes that heading.
    section: Vec<String>,
    /// Fence info string; serialized only for `"code_block"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<String>,
}

/// An in-progress top-level block being assembled from the event walk.
#[derive(Debug)]
enum Open {
    Heading {
        level: HeadingLevel,
        text: String,
        start: usize,
    },
    Code {
        lang: String,
        body: String,
        start: usize,
    },
    Source {
        type_name: &'static str,
        start: usize,
    },
}

impl Open {
    /// Begins a top-level block from a pulldown-cmark start tag, or `None`
    /// for tags that are not v1 block types (inlines, nested containers).
    fn from_tag(tag: &Tag<'_>, start: usize) -> Option<Self> {
        match tag {
            Tag::Heading { level, .. } => Some(Self::Heading {
                level: *level,
                text: String::new(),
                start,
            }),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.as_ref().to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                Some(Self::Code {
                    lang,
                    body: String::new(),
                    start,
                })
            }
            Tag::Paragraph => Some(Self::Source {
                type_name: "paragraph",
                start,
            }),
            Tag::List(_) => Some(Self::Source {
                type_name: "list",
                start,
            }),
            Tag::Table(_) => Some(Self::Source {
                type_name: "table",
                start,
            }),
            Tag::BlockQuote(_) => Some(Self::Source {
                type_name: "blockquote",
                start,
            }),
            Tag::HtmlBlock => Some(Self::Source {
                type_name: "html_block",
                start,
            }),
            _ => None,
        }
    }

    fn collect_text(&mut self, text: &str, inline_code: bool) {
        match self {
            Self::Heading { text: buf, .. } => buf.push_str(text),
            Self::Code { body, .. } if !inline_code => body.push_str(text),
            _ => {}
        }
    }

    fn finish(
        self,
        source: &str,
        end: usize,
        line_starts: &[usize],
        stack: &mut Vec<(u8, String)>,
    ) -> MdBlock {
        let start = match &self {
            Self::Heading { start, .. } | Self::Code { start, .. } | Self::Source { start, .. } => {
                *start
            }
        };
        let line = line_number(line_starts, start);
        match self {
            Self::Heading { level, text, .. } => {
                let level_num = heading_level_num(level);
                while stack
                    .last()
                    .is_some_and(|(open_level, _)| *open_level >= level_num)
                {
                    stack.pop();
                }
                stack.push((level_num, text.clone()));
                MdBlock {
                    block_type: level.to_string(),
                    content: text,
                    line,
                    section: section_path(stack),
                    lang: None,
                }
            }
            Self::Code { lang, body, .. } => MdBlock {
                block_type: "code_block".to_owned(),
                content: body,
                line,
                section: section_path(stack),
                lang: Some(lang),
            },
            Self::Source { type_name, start } => MdBlock {
                block_type: type_name.to_owned(),
                content: slice(source, start, end),
                line,
                section: section_path(stack),
                lang: None,
            },
        }
    }
}

/// CommonMark plus GFM tables; every other pulldown-cmark extension stays off.
fn parser_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options
}

/// Chunks markdown into a flat, document-ordered list of typed blocks.
///
/// Headings update a level stack; each block snapshots that stack as
/// `section`. Content before the first heading has an empty path. Parsing
/// never fails: pulldown-cmark accepts any input.
#[must_use]
pub(crate) fn chunk_markdown(source: &str) -> Vec<MdBlock> {
    let line_starts = line_starts(source);
    let mut blocks = Vec::new();
    let mut stack = Vec::new();
    let mut depth = 0usize;
    let mut open: Option<Open> = None;

    for (event, range) in Parser::new_ext(source, parser_options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    open = Open::from_tag(&tag, range.start);
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(current) = open.take()
                {
                    blocks.push(current.finish(source, range.end, &line_starts, &mut stack));
                }
            }
            Event::Text(text) => {
                if let Some(current) = open.as_mut() {
                    current.collect_text(&text, false);
                }
            }
            Event::Code(text) => {
                if let Some(current) = open.as_mut() {
                    current.collect_text(&text, true);
                }
            }
            Event::Rule if depth == 0 => {
                blocks.push(thematic_break(source, &range, &line_starts, &stack));
            }
            _ => {}
        }
    }
    blocks
}

/// Installs `md_to_json` as a persistent global valid for the section's whole
/// lifecycle. The function is pure: it captures nothing, so it needs no
/// observer, no budget, and no [`mlua::Scope`]. A non-string argument fails
/// through mlua's automatic type error.
///
/// # Errors
/// Returns [`Error::Lua`] if the function cannot be created or installed into
/// the sandbox globals.
pub(crate) fn install_md_to_json(lua: &Lua) -> Result<()> {
    let md_to_json = lua
        .create_function(|lua, source: String| lua.to_value(&chunk_markdown(&source)))
        .map_err(Error::lua)?;
    lua.globals()
        .raw_set("md_to_json", md_to_json)
        .map_err(Error::lua)
}

fn thematic_break(
    source: &str,
    range: &Range<usize>,
    line_starts: &[usize],
    stack: &[(u8, String)],
) -> MdBlock {
    MdBlock {
        block_type: "thematic_break".to_owned(),
        content: slice(source, range.start, range.end),
        line: line_number(line_starts, range.start),
        section: section_path(stack),
        lang: None,
    }
}

fn section_path(stack: &[(u8, String)]) -> Vec<String> {
    stack.iter().map(|(_, title)| title.clone()).collect()
}

fn slice(source: &str, start: usize, end: usize) -> String {
    source.get(start..end).unwrap_or("").to_owned()
}

/// Byte offsets of each 1-based line's first character, including a start at
/// 0 and one after every newline (so a trailing newline names an empty last
/// line).
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(source.len().saturating_div(32).saturating_add(1));
    starts.push(0);
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// 1-based line containing `offset`, via binary search on [`line_starts`].
fn line_number(starts: &[usize], offset: usize) -> u32 {
    let index = starts
        .partition_point(|&start| start <= offset)
        .saturating_sub(1);
    u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX)
}

fn heading_level_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::chunk_markdown;

    fn types(source: &str) -> Vec<String> {
        chunk_markdown(source)
            .into_iter()
            .map(|block| block.block_type)
            .collect()
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert!(chunk_markdown("").is_empty());
        assert!(chunk_markdown("\n\n").is_empty());
    }

    #[test]
    fn no_heading_input_has_empty_section_paths() {
        let blocks = chunk_markdown("just a paragraph\n\nand another\n");
        assert_eq!(
            types("just a paragraph\n\nand another\n"),
            ["paragraph", "paragraph"]
        );
        assert!(blocks.iter().all(|block| block.section.is_empty()));
        assert_eq!(blocks[0].content, "just a paragraph\n");
        assert_eq!(blocks[1].content, "and another\n");
        assert_eq!(blocks[0].line, 1);
        assert_eq!(blocks[1].line, 3);
    }

    #[test]
    fn preamble_before_first_heading_has_empty_section() {
        let blocks = chunk_markdown("before\n\n# H\n\nafter\n");
        assert_eq!(
            types("before\n\n# H\n\nafter\n"),
            ["paragraph", "h1", "paragraph"]
        );
        assert_eq!(blocks[0].section, [] as [String; 0]);
        assert_eq!(blocks[1].section, ["H"]);
        assert_eq!(blocks[2].section, ["H"]);
        assert_eq!(blocks[0].line, 1);
        assert_eq!(blocks[1].line, 3);
        assert_eq!(blocks[2].line, 5);
    }

    #[test]
    fn heading_hierarchy_snapshots_the_stack() {
        let source =
            "# 1 Introduction\n\nbody\n\n## 1.1 Motivation\n\nmore\n\n### Deep\n\n## 1.2 Next\n";
        let blocks = chunk_markdown(source);
        assert_eq!(
            types(source),
            ["h1", "paragraph", "h2", "paragraph", "h3", "h2"]
        );
        assert_eq!(blocks[0].content, "1 Introduction");
        assert_eq!(blocks[0].section, ["1 Introduction"]);
        assert_eq!(blocks[1].section, ["1 Introduction"]);
        assert_eq!(blocks[2].section, ["1 Introduction", "1.1 Motivation"]);
        assert_eq!(blocks[3].section, ["1 Introduction", "1.1 Motivation"]);
        assert_eq!(
            blocks[4].section,
            ["1 Introduction", "1.1 Motivation", "Deep"]
        );
        assert_eq!(blocks[5].section, ["1 Introduction", "1.2 Next"]);
    }

    #[test]
    fn skip_level_heading_does_not_invent_parents() {
        let blocks = chunk_markdown("# A\n\n### C\n");
        assert_eq!(blocks[0].section, ["A"]);
        assert_eq!(blocks[1].section, ["A", "C"]);
        assert_eq!(blocks[1].block_type, "h3");
    }

    #[test]
    fn adjacent_headings_have_no_body_between_them() {
        let blocks = chunk_markdown("# A\n## B\n");
        assert_eq!(types("# A\n## B\n"), ["h1", "h2"]);
        assert_eq!(blocks[0].content, "A");
        assert_eq!(blocks[1].content, "B");
        assert_eq!(blocks[0].line, 1);
        assert_eq!(blocks[1].line, 2);
        assert_eq!(blocks[1].section, ["A", "B"]);
    }

    #[test]
    fn heading_flattens_inline_markup() {
        let blocks = chunk_markdown("# Use `foo` and *bar*\n");
        assert_eq!(blocks[0].content, "Use foo and bar");
        assert_eq!(blocks[0].block_type, "h1");
    }

    #[test]
    fn fenced_code_with_and_without_lang() {
        let with_lang = chunk_markdown("```cpp\nint x; // c\n```\n");
        assert_eq!(with_lang[0].block_type, "code_block");
        assert_eq!(with_lang[0].lang.as_deref(), Some("cpp"));
        assert_eq!(with_lang[0].content, "int x; // c\n");
        assert_eq!(with_lang[0].line, 1);

        let no_lang = chunk_markdown("```\nfoo\n```\n");
        assert_eq!(no_lang[0].lang.as_deref(), Some(""));
        assert_eq!(no_lang[0].content, "foo\n");
    }

    #[test]
    fn indented_code_has_empty_lang_and_unindented_body() {
        let blocks = chunk_markdown("para\n\n    code\n    more\n");
        assert_eq!(
            types("para\n\n    code\n    more\n"),
            ["paragraph", "code_block"]
        );
        assert_eq!(blocks[1].lang.as_deref(), Some(""));
        assert_eq!(blocks[1].content, "code\nmore\n");
        assert_eq!(blocks[1].line, 3);
    }

    #[test]
    fn table_is_a_raw_source_slice() {
        let source = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["table"]);
        assert_eq!(blocks[0].content, source);
        assert_eq!(blocks[0].line, 1);
        assert!(blocks[0].lang.is_none());
    }

    #[test]
    fn list_is_one_block_and_does_not_break_out_items() {
        let source = "- a\n- b\n\npara\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["list", "paragraph"]);
        assert_eq!(blocks[0].content, "- a\n- b\n\n");
        assert_eq!(blocks[1].content, "para\n");
    }

    #[test]
    fn nested_list_stays_one_block() {
        let source = "- a\n  - b\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["list"]);
        assert_eq!(blocks[0].content, source);
    }

    #[test]
    fn code_inside_a_list_is_not_its_own_block() {
        let source = "- item\n\n      code\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["list"]);
        assert_eq!(blocks[0].content, source);
    }

    #[test]
    fn blockquote_is_the_raw_source_slice() {
        let source = "> hello\n>\n> world\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["blockquote"]);
        assert_eq!(blocks[0].content, source);
        assert_eq!(blocks[0].line, 1);
    }

    #[test]
    fn nested_heading_in_blockquote_does_not_update_the_stack() {
        let source = "> # Nested\n>\n> text\n\n# Real\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["blockquote", "h1"]);
        assert!(blocks[0].section.is_empty());
        assert_eq!(blocks[1].section, ["Real"]);
        assert_eq!(blocks[1].content, "Real");
    }

    #[test]
    fn thematic_break_is_its_own_block() {
        let source = "para\n\n---\n\nmore\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["paragraph", "thematic_break", "paragraph"]);
        assert_eq!(blocks[1].content, "---\n");
        assert_eq!(blocks[1].line, 3);
        assert_eq!(blocks[2].line, 5);
    }

    #[test]
    fn html_block_is_the_raw_source_slice() {
        let source = "<div>\nhi\n</div>\n\npara\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["html_block", "paragraph"]);
        assert_eq!(blocks[0].content, "<div>\nhi\n</div>\n");
        assert_eq!(blocks[0].line, 1);
        assert_eq!(blocks[1].line, 5);
    }

    #[test]
    fn paragraph_keeps_raw_inline_markdown() {
        let source = "a *b* `c` [d](e)\n";
        let blocks = chunk_markdown(source);
        assert_eq!(blocks[0].block_type, "paragraph");
        assert_eq!(blocks[0].content, source);
    }

    #[test]
    fn setext_heading_uses_inline_text_and_start_line() {
        let source = "Title\n=====\n\npara\n";
        let blocks = chunk_markdown(source);
        assert_eq!(types(source), ["h1", "paragraph"]);
        assert_eq!(blocks[0].content, "Title");
        assert_eq!(blocks[0].line, 1);
        assert_eq!(blocks[1].line, 4);
    }

    #[test]
    fn line_numbers_are_one_based_across_blank_lines() {
        let source = "# 1 Introduction\n\nWe propose a change to...\n\n```cpp\nint x;\n```\n";
        let blocks = chunk_markdown(source);
        assert_eq!(blocks[0].line, 1);
        assert_eq!(blocks[1].line, 3);
        assert_eq!(blocks[2].line, 5);
        assert_eq!(blocks[2].block_type, "code_block");
    }

    #[test]
    fn lang_is_absent_on_non_code_blocks() {
        let blocks = chunk_markdown("# H\n\npara\n\n---\n");
        assert!(blocks.iter().all(|block| block.lang.is_none()));
    }
}
