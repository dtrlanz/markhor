use std::ops::Range;
use pulldown_cmark::{html, CowStr, Event, HeadingLevel, OffsetIter, Options, Parser, Tag, TextMergeWithOffset};


#[derive(Debug)]
struct Markdown<'a> {
    content: CowStr<'a>,
}

impl<'a> Markdown<'a> {

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
