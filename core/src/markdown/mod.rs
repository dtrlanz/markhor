use std::fmt::{Debug, Display};
use std::ops::{Deref, Range};
use pulldown_cmark::{html, CowStr, HeadingLevel, Parser, Event, Tag, TagEnd};
use serde::Deserialize;

use crate::markdown::traversal::{TraversalEvent, Traversal};
use crate::markdown::xml::XmlTag;

mod xml;
mod to_markdown;
mod traversal;

pub use to_markdown::ToMarkdown;

#[derive(Debug, Clone)]
pub struct Markdown<'a> {
    pub content: &'a str,
    pub options: Options<'a>,
}

impl<'a> Markdown<'a> {
    pub fn options(&self) -> &Options<'a> {
        &self.options
    }

    pub fn options_mut(&mut self) -> &mut Options<'a> {
        &mut self.options
    }

    pub fn metadata<'b, T: Deserialize<'b>>(&self) -> Result<T, serde_yaml_ng::Error> 
        where 'a: 'b
    {
        let mut parser = self.parser().into_offset_iter();
        if let Some((Event::Start(Tag::MetadataBlock(..)), _)) = parser.next() {
        } else {
            return serde_yaml_ng::from_str("{}");
        };

        let mut start = 0;
        let mut stop = 0;

        while let Some((event, range)) = parser.next() {
            match event {
                Event::Text(_) => {
                    if start == 0 {
                        start = range.start;
                    }
                    stop = range.end;
                },
                Event::End(TagEnd::MetadataBlock(..)) => {
                    return serde_yaml_ng::from_str(&self.content[start..stop]);
                },
                _ => {},
            }
        }
        unreachable!()
    }

    pub fn skip_metadata(&self) -> Self {
        let mut parser = self.parser().into_offset_iter();
        if let Some((Event::Start(Tag::MetadataBlock(..)), _)) = parser.next() {
            let mut inside_yaml = true;
            while let Some((event, range)) = parser.next() {
                match event {
                    Event::End(TagEnd::MetadataBlock(..)) => {
                        inside_yaml = false;
                    },
                    Event::Start(_) if !inside_yaml => {
                        return Markdown {
                            content: &self.content[range.start..],
                            options: self.options.clone(),
                        };
                    },
                    _ => {},
                }
            }
        }
        self.clone()
    }

    pub fn sections(&self) -> Sections<'_> {
        Sections {
            iter: Traversal::new(self),
            open: Vec::new(),
            closed: Vec::new(),
        }
    }

    pub fn regions(&self) -> Regions<'_> {
        Regions {
            iter: Traversal::new(self),
            open: Vec::new(),
            closed: Vec::new(),
        }
    }

    pub fn prepend_milestone(&self, unit: &str, value: &str, attrs: Vec<(&'a str, Option<&'a str>)>) -> String {
        let mut attrs_str = String::new();
        for (attr_name, attr_value) in attrs {
            if let Some(attr_value) = attr_value {
                attrs_str.push_str(&format!(" {}=\"{}\"", attr_name, attr_value));
            } else {
                attrs_str.push_str(&format!(" {}", attr_name));
            }
        }
        let output = format!("<milestone unit=\"{}\" n=\"{}\"{} />\n{}", unit, value, attrs_str, self.content);
        output
    }

    fn parser(&self) -> Parser<'_> {
        Parser::new_ext(self.content, self.options.md_options)
    }

    pub fn to_html(&self) -> String {
        let mut html_buf = String::new();
        html::push_html(&mut html_buf, self.parser());
        html_buf
    }
}

impl<'a> PartialEq for Markdown<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.content == other.content
    }
}

impl<'a> AsRef<str> for Markdown<'a> {
    fn as_ref(&self) -> &str {
        self.content
    }
}

#[derive(Clone)]
pub struct Options<'a> {
    pub md_options: pulldown_cmark::Options,
    pub xml_filter: Option<&'a dyn Fn(&XmlTag<'_>) -> bool>,
    pub enable_milestones: bool,
}

impl<'a> Debug for Options<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraversalCfg")
            .field("xml_filter", &self.xml_filter.as_ref().map(|_| "Some(filter)"))
            .field("enable_milestones", &self.enable_milestones)
            .finish()
    }
}

pub const WITHOUT_XML: Options<'static> = Options {
    md_options: pulldown_cmark::Options::ENABLE_GFM
        .union(pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES)
        .union(pulldown_cmark::Options::ENABLE_YAML_STYLE_METADATA_BLOCKS),
    xml_filter: None,
    enable_milestones: false,
};

pub const WITH_MILESTONES: Options<'static> = Options {
    md_options: pulldown_cmark::Options::ENABLE_GFM
        .union(pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES)
        .union(pulldown_cmark::Options::ENABLE_YAML_STYLE_METADATA_BLOCKS),
    xml_filter: Some(&|tag: &XmlTag<'_>| 
        if let XmlTag::Empty { name, .. } = tag {
            *name == "milestone"
        } else {
            false
        }),
    enable_milestones: true,
};

#[derive(Clone)]
pub struct Section<'a> {
    md: Markdown<'a>,
    pub level: HeadingLevel,
    pub id: Option<CowStr<'a>>,
    pub classes: Vec<CowStr<'a>>,
    pub attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    pub range: Range<usize>,
}

impl<'a> Section<'a> {
    pub fn content(&self) -> &'a str {
        &self.md.content[self.range.clone()]
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

impl<'a> AsRef<Markdown<'a>> for Section<'a> {
    fn as_ref(&self) -> &Markdown<'a> {
        &self.md
    }
}

impl<'a> Deref for Section<'a> {
    type Target = Markdown<'a>;

    fn deref(&self) -> &Self::Target {
        &self.md
    }
}

pub struct Region<'a> {
    md: Markdown<'a>,
    pub unit: CowStr<'a>,
    pub value: CowStr<'a>,
    pub attrs: Vec<(&'a str, Option<CowStr<'a>>)>,
    pub range: Range<usize>,
}

impl<'a> Region<'a> {
    pub fn content(&self) -> &'a str {
        &self.md.content[self.range.clone()]
    }

    pub fn attribute(&self, name: &str) -> Option<Option<CowStr<'a>>> {
        for (attr_name, attr_value) in &self.attrs {
            if *attr_name == name {
                return Some(attr_value.clone());
            }
        }
        None
    }
}

impl<'a> Debug for Region<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Region")
            .field("unit", &self.unit)
            .field("value", &self.value)
            .field("attrs", &self.attrs)
            .field("range", &self.range)
            .finish()
    }
}

impl<'a> Display for Region<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.content())
    }
}

impl<'a> PartialEq for Region<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.content() == other.content()
    }
}

impl<'a> AsRef<Markdown<'a>> for Region<'a> {
    fn as_ref(&self) -> &Markdown<'a> {
        &self.md
    }
}

impl<'a> Deref for Region<'a> {
    type Target = Markdown<'a>;

    fn deref(&self) -> &Self::Target {
        &self.md
    }
    
}

pub struct Sections<'a> {
    iter: Traversal<'a>,
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
                        md: Markdown { 
                            content: "",
                            options: self.iter.md.options.clone(),
                        },
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
                        // End of previous section equals start of next section
                        section.range.end = range.start;
                        section.md.content = &self.iter.md.content[section.range.clone()];
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

pub struct Regions<'a> {
    iter: Traversal<'a>,
    open: Vec<Region<'a>>,
    closed: Vec<Region<'a>>,
}

impl<'a> Regions<'a> {
    fn next_region_to_yield(&mut self) -> Option<Region<'a>> {
        if self.closed.len() == 0 {
            return None;
        }

        // Find earliest closed region (order first by start pos, then by end pos)
        let mut earliest_index = 0;
        for (i, region) in self.closed.iter().enumerate().skip(1) {
            let earliest = &self.closed[earliest_index];
            if region.range.start < earliest.range.start ||
               (region.range.start == earliest.range.start && region.range.end < earliest.range.end) {
                earliest_index = i;
            }
        }
        
        // Check if any currently open region started before this earliest closed region
        let earliest_closed = &self.closed[earliest_index];
        for region in &self.open {
            if region.range.start < earliest_closed.range.start {
                return None; // An open region started earlier; yield it first
            }
        }

        Some(self.closed.remove(earliest_index))
    }
}

impl<'a> Iterator for Regions<'a> {
    type Item = Region<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(region) = self.next_region_to_yield() {
            return Some(region);
        }

        while let Some((event, range)) = self.iter.next() {
            match event {
                TraversalEvent::RegionStart { unit, value, attributes } => {
                    let region = Region {
                        md: Markdown { 
                            content: "",
                            options: self.iter.md.options.clone(),
                        },
                        unit,
                        value,
                        attrs: attributes,
                        // Region starts *after* milestone; range end will be updated on RegionEnd
                        range: range.end..range.end,
                    };
                    self.open.push(region);
                },
                TraversalEvent::RegionEnd { unit } => {
                    let unit_match = self.open.extract_if(.., |r| r.unit == unit ).nth(0);
                    if let Some(mut region) = unit_match {
                        // Previous region ends *before* milestone
                        region.range.end = range.start;
                        region.md.content = &self.iter.md.content[region.range.clone()];
                        self.closed.push(region);
                        if let Some(next_region) = self.next_region_to_yield() {
                            return Some(next_region);
                        }
                    } else {
                        panic!("Mismatched region end");
                    }
                },
                _ => {},
            }
        }
        assert_eq!(self.open.len(), 0, "Unclosed regions remain");
        assert_eq!(self.closed.len(), 0, "Unyielded regions remain");
        None
    }
}


#[cfg(test)]
mod tests {
    use serde_yaml_ng::Value;

    use crate::markdown::to_markdown::ToMarkdown;

    use super::*;

    #[test]
    fn sections() {
        let text = r#"# Heading 1

Some paragraph text.

## Heading 2

More text.

# Heading 1

And more."#;

        let md = text.to_markdown(WITHOUT_XML);
        let sections: Vec<Section> = md.sections().collect();

        assert_eq!(sections.len(), 3);
        println!("{:#?}", sections[0]);
        assert_eq!(sections[0].level, HeadingLevel::H1);
        assert_eq!(&text[sections[0].range.clone()], "# Heading 1\n\nSome paragraph text.\n\n## Heading 2\n\nMore text.\n\n");
        println!("{:#?}", sections[1]);
        assert_eq!(sections[1].level, HeadingLevel::H2);
        assert_eq!(&text[sections[1].range.clone()], "## Heading 2\n\nMore text.\n\n");
        assert_eq!(sections[1].to_html(), "<h2>Heading 2</h2>\n<p>More text.</p>\n");
        println!("{:#?}", sections[2]);
        assert_eq!(sections[2].level, HeadingLevel::H1);
        assert_eq!(&text[sections[2].range.clone()], "# Heading 1\n\nAnd more.");
        assert_eq!(sections[2].to_html(), "<h1>Heading 1</h1>\n<p>And more.</p>\n");

    }

    #[test]
    fn regions() {
        let text = r#"Some text.
<milestone unit="part" n="1"/>
More text.
<milestone unit="part" n="2"/>
And more."#;

        let md = text.to_markdown(WITH_MILESTONES);

        let regions: Vec<Region> = md.regions().collect();

        assert_eq!(regions.len(), 2);
        println!("{:#?}", regions[0]);
        assert_eq!(regions[0].unit, CowStr::from("part"));
        assert_eq!(regions[0].value, CowStr::from("1"));
        assert_eq!(&text[regions[0].range.clone()], "\nMore text.\n");
        assert_eq!(regions[0].to_html(), "<p>More text.</p>\n");
        println!("{:#?}", regions[1]);
        assert_eq!(regions[1].unit, CowStr::from("part"));
        assert_eq!(regions[1].value, CowStr::from("2"));
        assert_eq!(&text[regions[1].range.clone()], "\nAnd more.");
        assert_eq!(regions[1].to_html(), "<p>And more.</p>\n");
    }

    #[test]
    fn metadata_some() {
        let text = r#"---
title: Sample Document
author: Test Author
---
Some text."#;

        let md = text.to_markdown(WITHOUT_XML);

        let metadata: Value = md.metadata().unwrap();
        assert_eq!(metadata["title"], "Sample Document");
        assert_eq!(metadata["author"], "Test Author");

        let skipped = md.skip_metadata();
        assert_eq!(skipped.content, "Some text.");

        #[derive(Debug, Deserialize)]
        struct DocInfo {
            title: String,
            author: String,
        }

        let metadata_typed: Result<DocInfo, _> = md.metadata();
        let metadata_typed = metadata_typed.unwrap();
        assert_eq!(metadata_typed.title, "Sample Document");
        assert_eq!(metadata_typed.author, "Test Author");
    }

    #[test]
    fn metadata_none() {

        let text = r#"Some text without metadata."#;
        let md = text.to_markdown(WITHOUT_XML);
        let metadata: Value = md.metadata().unwrap();
        assert_eq!(metadata, Value::Mapping(serde_yaml_ng::Mapping::new()));

        let skipped = md.skip_metadata();
        assert_eq!(skipped.content, text);
    }

}