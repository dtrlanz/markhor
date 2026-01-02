use std::fmt::{Debug, Display};
use std::ops::Range;
use pulldown_cmark::{html, CowStr, Event, HeadingLevel, OffsetIter, Options, Parser, Tag, TextMergeWithOffset};

use crate::markdown::traversal::{WITHOUT_XML, TraversalCfg, TraversalEvent, Traverse};

mod xml;
mod markdown;
mod traversal;

#[derive(Debug)]
struct Markdown<'a> {
    content: CowStr<'a>,
}

impl<'a> Markdown<'a> {
    pub(crate) fn traverse<'b>(&'b self, cfg: TraversalCfg<'b>) -> Traverse<'b> {
        Traverse::new(&self.content, cfg)
    }

    pub fn sections(&self) -> Sections<'_> {
        Sections {
            iter: self.traverse(WITHOUT_XML),
            open: Vec::new(),
            closed: Vec::new(),
        }
    }

    fn parser(&self) -> Parser {
        let parser_options: Options = [
            Options::ENABLE_GFM,
            Options::ENABLE_HEADING_ATTRIBUTES
        ].into_iter().collect();

        Parser::new_ext(&*self.content, parser_options)
    }

    fn to_html(&self) -> String {
        let mut html_buf = String::new();
        html::push_html(&mut html_buf, self.parser());
        html_buf
    }
}

impl<'a, T: Into<CowStr<'a>>,> From<T> for Markdown<'a> {
    fn from(content: T) -> Self {
        Markdown {
            content: content.into(),
        }
    }
}

#[derive(Eq, Clone)]
struct Section<'a> {
    source_str: &'a str,
    level: HeadingLevel,
    id: Option<CowStr<'a>>,
    classes: Vec<CowStr<'a>>,
    attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    range: Range<usize>,
}

impl<'a> Section<'a> {
    pub fn content(&self) -> &'a str {
        &self.source_str[self.range.clone()]
    }
}

impl<'a> Debug for Section<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Section")
            .field("level", &self.level)
            .field("id", &self.id)
            .field("classes", &self.classes)
            .field("attrs", &self.attrs)
            .field("range", &self.range)
            .finish()
    }
}

impl<'a> Display for Section<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.content())
    }
}

impl<'a> PartialEq for Section<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.content() == other.content()
    }
}

struct Sections<'a> {
    iter: Traverse<'a>,
    open: Vec<Section<'a>>,
    closed: Vec<Section<'a>>,
}

impl<'a> Iterator for Sections<'a> {
    type Item = Section<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(section) = self.closed.pop() {
            return Some(section);
        }

        while let Some((event, range)) = self.iter.next() {
            match event {
                TraversalEvent::SectionStart { level, id, classes, attrs } => {
                    let section = Section {
                        source_str: &self.iter.content,
                        level,
                        id,
                        classes,
                        attrs,
                        range: range,
                    };
                    self.open.push(section);
                },
                TraversalEvent::SectionEnd { level } => {
                    if let Some(mut section) = self.open.pop() {
                        assert_eq!(section.level, level);
                        section.range.end = range.end;
                        if self.open.len() == 0 {
                            return Some(section);
                        } else {
                            // Pre-order traversal: Children should be yielded after their 
                            // parents. Since children are closed first, we push them onto a
                            // stack to yield after the parent is yielded.
                            self.closed.push(section);
                            continue;
                        }
                    }
                    panic!("Mismatched section end");
                },
                _ => {},
            }
        }
        assert_eq!(self.open.len(), 0, "Unclosed sections remain");
        None
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections() {
        const text: &str = r#"# Heading 1

Some paragraph text.

## Heading 2

More text."#;

        let md = Markdown::from(text);
        let sections: Vec<Section> = md.sections().collect();

        assert_eq!(sections.len(), 2);
        println!("{:#?}", sections[0]);
        assert_eq!(sections[0].level, HeadingLevel::H1);
        assert_eq!(&sections[0].source_str[sections[0].range.clone()], "# Heading 1\n\nSome paragraph text.\n\n## Heading 2\n\nMore text.");
        println!("{:#?}", sections[1]);
        assert_eq!(sections[1].level, HeadingLevel::H2);
        assert_eq!(&sections[1].source_str[sections[1].range.clone()], "## Heading 2\n\nMore text.");

    }
}