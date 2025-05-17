use std::ops::Range;

use pulldown_cmark::{html, CowStr, Event, HeadingLevel, OffsetIter, Options, Parser, Tag, TextMergeWithOffset};

#[derive(Debug)]
struct Markdown<'a> {
    content: CowStr<'a>,
}

impl<'a> Markdown<'a> {
    fn sections(&self) -> Sections<'_> {
        let mut heading_level = None;
        let mut section_ranges = Vec::new();

        let mut section_start = 0;

        for (event, range) in self.parser().into_offset_iter() {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    if let Some(current_top) = heading_level {
                        if level < current_top {
                            heading_level = Some(level);
                            section_start = 0;
                            section_ranges.truncate(0);
                        }
                    } else {
                        heading_level = Some(level);
                    }
                    if section_start != range.start {
                        section_ranges.push(section_start..range.start);
                        section_start = range.start;
                    }
                }
                _ => {}
            }
        }
        section_ranges.push(section_start..self.content.len());
        println!("Sections: {:?}", section_ranges);
        // Use as queue, not stack
        section_ranges.reverse();

        Sections {
            content: &*self.content,
            heading_level,
            section_ranges,
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

struct Sections<'a> {
    content: &'a str,
    heading_level: Option<HeadingLevel>,
    section_ranges: Vec<Range<usize>>,
}

impl<'a> Sections<'a> {
    pub fn heading_level(&self) -> Option<HeadingLevel> {
        self.heading_level
    }
}

impl<'a> Iterator for Sections<'a> {
    type Item = Markdown<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(range) = self.section_ranges.pop() {
            Some(self.content[range].into())
        } else {
            None
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_lvl1_lvl1_with_leading_text() {
        let markdown = Markdown::from(
"Text without heading

# 1
Text under 1

# 2
Text under 2");

        let sections = markdown.sections();
        assert_eq!(sections.heading_level(), Some(HeadingLevel::H1));

        let sections: Vec<_> = sections
            .map(|md| md.to_html())
            .collect::<Vec<_>>();

        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0], "<p>Text without heading</p>\n");
        assert_eq!(sections[1], "<h1>1</h1>\n<p>Text under 1</p>\n");
        assert_eq!(sections[2], "<h1>2</h1>\n<p>Text under 2</p>\n");
    }

    #[test]
    fn headings_lvl1_lvl1_without_leading_text() {
        let markdown = Markdown::from(
"# 1
Text under 1

# 2
Text under 2");

        let sections = markdown.sections();
        assert_eq!(sections.heading_level(), Some(HeadingLevel::H1));

        let sections: Vec<_> = sections
            .map(|md| md.to_html())
            .collect::<Vec<_>>();

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0], "<h1>1</h1>\n<p>Text under 1</p>\n");
        assert_eq!(sections[1], "<h1>2</h1>\n<p>Text under 2</p>\n");
    }

    #[test]
    fn headings_lvl2_lvl1() {
        let markdown = Markdown::from(
"Text without heading

## 0.1
Text under 0.1

# 1
Text under 1");

        let sections = markdown.sections();
        assert_eq!(sections.heading_level(), Some(HeadingLevel::H1));

        let sections: Vec<_> = sections
            .map(|md| md.to_html())
            .collect::<Vec<_>>();

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0], "<p>Text without heading</p>\n<h2>0.1</h2>\n<p>Text under 0.1</p>\n");
        assert_eq!(sections[1], "<h1>1</h1>\n<p>Text under 1</p>\n");
    }

    #[test]
    fn headings_lvl3_lvl3() {
        let markdown = Markdown::from(
"### 0.0.1
Text under 0.0.1

### 0.0.2
Text under 0.0.2");

        let sections = markdown.sections();
        assert_eq!(sections.heading_level(), Some(HeadingLevel::H3));

        let sections: Vec<_> = sections
            .map(|md| md.to_html())
            .collect::<Vec<_>>();

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0], "<h3>0.0.1</h3>\n<p>Text under 0.0.1</p>\n");
        assert_eq!(sections[1], "<h3>0.0.2</h3>\n<p>Text under 0.0.2</p>\n");
    }
}