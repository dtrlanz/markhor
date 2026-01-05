use std::fmt::{Debug, Display};
use std::ops::Range;
use pulldown_cmark::{html, CowStr, Event, HeadingLevel, OffsetIter, Parser, Tag, TextMergeWithOffset};

use crate::markdown::traversal::{Options, TraversalEvent, Traverse, WITH_MILESTONES, WITHOUT_XML};

mod xml;
mod markdown;
mod traversal;

#[derive(Debug)]
pub struct Markdown<'a> {
    content: &'a str,
    options: Options<'a>,
}

impl<'a> Markdown<'a> {
    pub fn options(&self) -> &Options<'a> {
        &self.options
    }

    pub fn options_mut(&mut self) -> &mut Options<'a> {
        &mut self.options
    }

    pub fn sections(&self) -> Sections<'_> {
        Sections {
            iter: Traverse::new(&self.content, &self.options),
            open: Vec::new(),
            closed: Vec::new(),
        }
    }

    pub fn regions(&self) -> Regions<'_> {
        Regions {
            iter: Traverse::new(&self.content, &self.options),
            open: Vec::new(),
            closed: Vec::new(),
        }
    }

    pub fn parser(&self) -> Parser<'_> {
        Parser::new_ext(self.content, self.options.md_options)
    }

    pub fn to_html(&self) -> String {
        let mut html_buf = String::new();
        html::push_html(&mut html_buf, self.parser());
        html_buf
    }
}

impl<'a> From<&'a str> for Markdown<'a> {
    fn from(content: &'a str) -> Self {
        Markdown {
            content: content,
            options: WITH_MILESTONES,
        }
    }
}

#[derive(Eq, Clone)]
struct Section<'a> {
    source_str: &'a str,
    pub level: HeadingLevel,
    pub id: Option<CowStr<'a>>,
    pub classes: Vec<CowStr<'a>>,
    pub attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    pub range: Range<usize>,
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

struct Region<'a> {
    source_str: &'a str,
    pub unit: CowStr<'a>,
    pub value: CowStr<'a>,
    pub attrs: Vec<(&'a str, Option<CowStr<'a>>)>,
    pub range: Range<usize>,
}

impl<'a> Region<'a> {
    pub fn content(&self) -> &'a str {
        &self.source_str[self.range.clone()]
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
                        // End of previous section equals start of next section
                        section.range.end = range.start;
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

struct Regions<'a> {
    iter: Traverse<'a>,
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
                        source_str: &self.iter.content,
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
    use std::borrow::Cow;

    use super::*;

    #[test]
    fn sections() {
        let text = r#"# Heading 1

Some paragraph text.

## Heading 2

More text.

# Heading 1

And more."#;

        let md = Markdown::from(text);
        let sections: Vec<Section> = md.sections().collect();

        assert_eq!(sections.len(), 3);
        println!("{:#?}", sections[0]);
        assert_eq!(sections[0].level, HeadingLevel::H1);
        assert_eq!(&text[sections[0].range.clone()], "# Heading 1\n\nSome paragraph text.\n\n## Heading 2\n\nMore text.\n\n");
        println!("{:#?}", sections[1]);
        assert_eq!(sections[1].level, HeadingLevel::H2);
        assert_eq!(&text[sections[1].range.clone()], "## Heading 2\n\nMore text.\n\n");
        println!("{:#?}", sections[2]);
        assert_eq!(sections[2].level, HeadingLevel::H1);
        assert_eq!(&text[sections[2].range.clone()], "# Heading 1\n\nAnd more.");

    }

    #[test]
    fn regions() {
        let text = r#"Some text.
<milestone unit="part" n="1"/>
More text.
<milestone unit="part" n="2"/>
And more."#;

        let md = Markdown::from(text);

        let regions: Vec<Region> = md.regions().collect();

        assert_eq!(regions.len(), 2);
        println!("{:#?}", regions[0]);
        assert_eq!(regions[0].unit, CowStr::from("part"));
        assert_eq!(regions[0].value, CowStr::from("1"));
        assert_eq!(&text[regions[0].range.clone()], "\nMore text.\n");
        println!("{:#?}", regions[1]);
        assert_eq!(regions[1].unit, CowStr::from("part"));
        assert_eq!(regions[1].value, CowStr::from("2"));
        assert_eq!(&text[regions[1].range.clone()], "\nAnd more.");

    }
}