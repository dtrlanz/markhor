use crate::markdown::{Markdown, Options};

pub trait ToMarkdown {
    fn to_markdown<'a>(&'a self, options: Options<'a>) -> Markdown<'a>;
}

impl<T: AsRef<str>> ToMarkdown for T {
    fn to_markdown<'a>(&'a self, options: Options<'a>) -> Markdown<'a> {
        Markdown {
            content: self.as_ref(),
            options,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::WITHOUT_XML;

    #[test]
    fn to_markdown() {
        let text = r#"Some **text**"#;
        let md = text.to_markdown(WITHOUT_XML);

        assert_eq!(md.content, text);
        assert_eq!(md.to_html(), "<p>Some <strong>text</strong></p>\n");

        let text2 = String::from(text);
        let md2 = text2.to_markdown(WITHOUT_XML);

        assert_eq!(md2.content, text);
        assert_eq!(md2.to_html(), "<p>Some <strong>text</strong></p>\n");
    }
}