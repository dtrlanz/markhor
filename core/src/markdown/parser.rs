//! Extension to the Markdown parser `pulldown_cmark`
//! 
//! This module provides a custom Markdown parser that extends the functionality of the 
//! `pulldown_cmark` library to support XML tags.
//! 
//! It customizes the `Tag` enum to include XML tags and implements a custom `Parser` that can 
//! handle these tags.
//! 
//! The enums `Event`, `Tag`, and `TagEnd` are derived from the enums of the same names in the
//! `pulldown_cmark` library, with additional variants for XML tags.

use std::ops::Range;

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, CowStr, HeadingLevel, LinkType, MetadataBlockKind, OffsetIter};

use super::xml::{iter_tags_with_context, TagWithContext, XmlTag};


struct Parser<'a> {
    text: &'a str,
    xml_stack: Vec<(XmlTag<'a>, Range<usize>)>,
    md_parser: Option<(usize, OffsetIter<'a>)>,
}

impl<'a> Parser<'a> {
    pub fn with_tag_filter<F: FnMut(&XmlTag<'_>) -> bool>(text: &'a str, mut tag_filter: F) -> Self {
        let mut xml_stack = Vec::new();
        let mut md_parser = pulldown_cmark::Parser::new(text).into_offset_iter();

        // Parse the text to find XML tags
        while let Some((event, range)) = md_parser.next() {
            match event {
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::HtmlBlock) |
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::Paragraph) => {
                    for item in iter_tags_with_context(&text[range]) {
                        let TagWithContext { tag, range, before, after } = item;
                        match tag {
                            XmlTag::Start { .. } => if before.chars().any(|c| !c.is_whitespace()) {
                                // Start tag must be at the beginning of a line
                                continue;
                            },
                            XmlTag::End { .. } => {
                                if after.chars().any(|c| !c.is_whitespace()) {
                                    // End tag must be at the end of a line
                                    continue;
                                }
                                // Look for matching start tag
                                let mut closed_count = 1u32;
                                for i in (0..xml_stack.len()).rev() {
                                    match xml_stack[i].0 {
                                        XmlTag::Start { name, .. } if name == tag.name => {
                                            closed_count -= 1;
                                        }
                                        XmlTag::End { .. } if name == tag.name => {
                                            closed_count += 1;
                                        }
                                        _ => {}
                                    }
                                    if closed_count == 0 {
                                        // Found matching start tag

                                        fn find_end_tag(
                                            xml_stack: &[(XmlTag<'_>, Range<usize>)],
                                        ) -> Option<usize> {
                                            let mut idx = 0;
                                            while idx < xml_stack.len() {
                                                

                                        }

                                        // Check if any intervening start tags are unclosed
                                        let mut open_count = 0u32;
                                        for j in i..xml_stack.len() {
                                        }

                                        break;
                                    }
                                }
                                    
                            },
                            _ => {},
                        }
                        if !tag_filter(&tag) {
                            continue;
                        }
                        xml_stack.push((tag, range));
                    }

                }
                _ => {}
            }
        }

        // Initialize the Markdown parser with text before the first XML tag
        // (or the whole text if no tags)
        let initial_content = if let Some((_, first_xml)) = xml_stack.first() {
            &text[..first_xml.start]
        } else {
            text
        };
        let md_parser = pulldown_cmark::Parser::new(initial_content).into_offset_iter();

        // Pop XML tags in FIFO order
        xml_stack.reverse();

        Parser {
            text,
            xml_stack,
            md_parser: Some((0, md_parser)),
        }
    }
}

impl<'a> Iterator for Parser<'a> {
    type Item = (Event<'a>, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        // Parse text before the next XML tag
        if let Some((offset, mut iter)) = self.md_parser.take() {
            if let Some((event, range)) = iter.next() {
                let range = Range {
                    start: offset + range.start,
                    end: offset + range.end,
                };
                self.md_parser = Some((offset, iter));
                return Some((Event::from(event), range));
            }
        }

        // Parse XML tag
        if let Some((tag, range)) = self.xml_stack.pop() {
            let (event, elem_range, continue_offset) = match tag {
                XmlTag::Start{ name, attributes } => {
                    // Find range from XML start tag to end tag
                    let (_, end_tag_range) = self.xml_stack.iter_mut()
                        .rfind(|(t, _)| match t {
                            XmlTag::End { name: n } => *n == name,
                            _ => false,
                        }).unwrap();
                    let elem_range = range.start..end_tag_range.end;

                    // Store start of opening tag to be returned later with the event of the 
                    // closing tag. This allows behavior consistent with other Markdown blocks 
                    // (e.g., paragraphs).
                    end_tag_range.start = range.start;

                    let event = Event::Start(Tag::Xml {
                        name,
                        attrs: attributes,
                    });

                    (event, elem_range, range.end)
                }
                XmlTag::Empty { name, attributes } => {
                    let event = Event::Start(Tag::Xml {
                        name,
                        attrs: attributes,
                    });
                    let range_end = range.end;
                    (event, range, range_end)
                    // TODO return corresponding Event::End
                }
                XmlTag::End { .. } => {
                    let event = Event::End(TagEnd::Xml);
                    let range_end = range.end;
                    (event, range, range_end)
                }
            };

            // Prepare to parse text before next XML tag
            let content = if let Some((_, next_xml)) = self.xml_stack.last_mut() {
                // Parse text between the current XML tag and the next one, if any
                &self.text[continue_offset..next_xml.start]
            } else {
                // If no more XML tags, parse the rest of the text
                &self.text[continue_offset..]
            };
            self.md_parser = Some((
                continue_offset,
                pulldown_cmark::Parser::new(content).into_offset_iter(),
            ));

            return Some((event, elem_range));
        }

        None
    }
}

/// Tags for elements that can contain other elements.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag<'a> {
    // Existing variants as per `pulldown_cmark`
    Paragraph,
    Heading {
        level: HeadingLevel,
        id: Option<CowStr<'a>>,
        classes: Vec<CowStr<'a>>,
        attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    },
    BlockQuote(Option<BlockQuoteKind>),
    CodeBlock(CodeBlockKind<'a>),
    HtmlBlock,
    List(Option<u64>),
    Item,
    FootnoteDefinition(CowStr<'a>),
    DefinitionList,
    DefinitionListTitle,
    DefinitionListDefinition,
    Table(Vec<Alignment>),
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    Superscript,
    Subscript,
    Link {
        link_type: LinkType,
        dest_url: CowStr<'a>,
        title: CowStr<'a>,
        id: CowStr<'a>,
    },
    Image {
        link_type: LinkType,
        dest_url: CowStr<'a>,
        title: CowStr<'a>,
        id: CowStr<'a>,
    },
    MetadataBlock(MetadataBlockKind),
    // Custom variant for XML tags
    Xml {
        name: CowStr<'a>,
        attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    },
}

impl<'a> From<pulldown_cmark::Tag<'a>> for Tag<'a> {
    fn from(tag: pulldown_cmark::Tag<'a>) -> Self {
        match tag {
            pulldown_cmark::Tag::Paragraph => Tag::Paragraph,
            pulldown_cmark::Tag::Heading {
                level,
                id,
                classes,
                attrs,
            } => Tag::Heading {
                level,
                id: id.map(CowStr::from),
                classes: classes.into_iter().map(CowStr::from).collect(),
                attrs: attrs
                    .into_iter()
                    .map(|(k, v)| (CowStr::from(k), v.map(CowStr::from)))
                    .collect(),
            },
            pulldown_cmark::Tag::BlockQuote(kind) => Tag::BlockQuote(kind.map(Into::into)),
            pulldown_cmark::Tag::CodeBlock(kind) => Tag::CodeBlock(kind.into()),
            pulldown_cmark::Tag::HtmlBlock => Tag::HtmlBlock,
            pulldown_cmark::Tag::List(start) => Tag::List(start),
            pulldown_cmark::Tag::Item => Tag::Item,
            pulldown_cmark::Tag::FootnoteDefinition(id) => Tag::FootnoteDefinition(CowStr::from(id)),
            pulldown_cmark::Tag::DefinitionList => Tag::DefinitionList,
            pulldown_cmark::Tag::DefinitionListTitle => Tag::DefinitionListTitle,
            pulldown_cmark::Tag::DefinitionListDefinition => Tag::DefinitionListDefinition,
            pulldown_cmark::Tag::Table(alignment) => Tag::Table(alignment.into_iter().map(Into::into).collect()),
            pulldown_cmark::Tag::TableHead => Tag::TableHead,
            pulldown_cmark::Tag::TableRow => Tag::TableRow,
            pulldown_cmark::Tag::TableCell => Tag::TableCell,
            pulldown_cmark::Tag::Emphasis => Tag::Emphasis,
            pulldown_cmark::Tag::Strong => Tag::Strong,
            pulldown_cmark::Tag::Strikethrough => Tag::Strikethrough,
            pulldown_cmark::Tag::Superscript => Tag::Superscript,
            pulldown_cmark::Tag::Subscript => Tag::Subscript,
            pulldown_cmark::Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            } => Tag::Link {
                link_type,
                dest_url: CowStr::from(dest_url),
                title: CowStr::from(title),
                id: CowStr::from(id),
            },
            pulldown_cmark::Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            } => Tag::Image {
                link_type,
                dest_url: CowStr::from(dest_url),
                title: CowStr::from(title),
                id: CowStr::from(id),
            },
            pulldown_cmark::Tag::MetadataBlock(kind) => Tag::MetadataBlock(kind.into()),
        }
    }
}

/// The end of a `Tag`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum TagEnd {
    // Existing variants as per `pulldown_cmark`
    Paragraph,
    Heading(HeadingLevel),
    BlockQuote(Option<BlockQuoteKind>),
    CodeBlock,
    HtmlBlock,
    List(bool),
    Item,
    FootnoteDefinition,
    DefinitionList,
    DefinitionListTitle,
    DefinitionListDefinition,
    Table,
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    Superscript,
    Subscript,
    Link,
    Image,
    MetadataBlock(MetadataBlockKind),
    // Custom variant for XML tags
    Xml,
}

impl From<pulldown_cmark::TagEnd> for TagEnd {
    fn from(tag_end: pulldown_cmark::TagEnd) -> Self {
        match tag_end {
            pulldown_cmark::TagEnd::Paragraph => TagEnd::Paragraph,
            pulldown_cmark::TagEnd::Heading(level) => TagEnd::Heading(level),
            pulldown_cmark::TagEnd::BlockQuote(kind) => TagEnd::BlockQuote(kind.map(Into::into)),
            pulldown_cmark::TagEnd::CodeBlock => TagEnd::CodeBlock,
            pulldown_cmark::TagEnd::HtmlBlock => TagEnd::HtmlBlock,
            pulldown_cmark::TagEnd::List(start) => TagEnd::List(start),
            pulldown_cmark::TagEnd::Item => TagEnd::Item,
            pulldown_cmark::TagEnd::FootnoteDefinition => TagEnd::FootnoteDefinition,
            pulldown_cmark::TagEnd::DefinitionList => TagEnd::DefinitionList,
            pulldown_cmark::TagEnd::DefinitionListTitle => TagEnd::DefinitionListTitle,
            pulldown_cmark::TagEnd::DefinitionListDefinition => TagEnd::DefinitionListDefinition,
            pulldown_cmark::TagEnd::Table => TagEnd::Table,
            pulldown_cmark::TagEnd::TableHead => TagEnd::TableHead,
            pulldown_cmark::TagEnd::TableRow => TagEnd::TableRow,
            pulldown_cmark::TagEnd::TableCell => TagEnd::TableCell,
            pulldown_cmark::TagEnd::Emphasis => TagEnd::Emphasis,
            pulldown_cmark::TagEnd::Strong => TagEnd::Strong,
            pulldown_cmark::TagEnd::Strikethrough => TagEnd::Strikethrough,
            pulldown_cmark::TagEnd::Superscript => TagEnd::Superscript,
            pulldown_cmark::TagEnd::Subscript => TagEnd::Subscript,
            pulldown_cmark::TagEnd::Link => TagEnd::Link,
            pulldown_cmark::TagEnd::Image => TagEnd::Image,
            pulldown_cmark::TagEnd::MetadataBlock(kind) => TagEnd::MetadataBlock(kind.into()),
        }
    }
}



pub enum Event<'a> {
    Start(Tag<'a>),
    End(TagEnd),
    Text(CowStr<'a>),
    Code(CowStr<'a>),
    InlineMath(CowStr<'a>),
    DisplayMath(CowStr<'a>),
    Html(CowStr<'a>),
    InlineHtml(CowStr<'a>),
    FootnoteReference(CowStr<'a>),
    SoftBreak,
    HardBreak,
    Rule,
    TaskListMarker(bool),
}

impl<'a> From<pulldown_cmark::Event<'a>> for Event<'a> {
    fn from(event: pulldown_cmark::Event<'a>) -> Self {
        match event {
            pulldown_cmark::Event::Start(tag) => Event::Start(tag.into()),
            pulldown_cmark::Event::End(tag_end) => Event::End(tag_end.into()),
            pulldown_cmark::Event::Text(text) => Event::Text(CowStr::from(text)),
            pulldown_cmark::Event::Code(code) => Event::Code(CowStr::from(code)),
            pulldown_cmark::Event::InlineMath(math) => Event::InlineMath(CowStr::from(math)),
            pulldown_cmark::Event::DisplayMath(math) => Event::DisplayMath(CowStr::from(math)),
            pulldown_cmark::Event::Html(html) => Event::Html(CowStr::from(html)),
            pulldown_cmark::Event::InlineHtml(html) => Event::InlineHtml(CowStr::from(html)),
            pulldown_cmark::Event::FootnoteReference(id) => Event::FootnoteReference(CowStr::from(id)),
            pulldown_cmark::Event::SoftBreak => Event::SoftBreak,
            pulldown_cmark::Event::HardBreak => Event::HardBreak,
            pulldown_cmark::Event::Rule => Event::Rule,
            pulldown_cmark::Event::TaskListMarker(checked) => Event::TaskListMarker(checked),
        }
    }
}