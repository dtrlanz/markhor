use std::ops::{Deref, DerefMut};

pub enum Content2 {
    Blob(Blob),
    Text(Text),
}

pub struct Blob {
    // TODO
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Text {
    text_parts: Vec<(String, String)>,
}


impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> Option<&str> {
        self.text_parts.first().map(|(_, text)| text.as_ref())
    }

    pub fn text_mut(&mut self) -> TextMut {
        if self.text_parts.len() > 0 {
            let text = &mut self.text_parts[0].1;
            TextMut::occupied(text)
        } else {
            TextMut::vacant(&mut self.text_parts, "")
        }
    }

    pub fn text_by_id(&self, id: &str) -> Option<&str> {
        self.text_parts.iter().find(|(part_id, _)| part_id == id).map(|(_, text)| text.as_ref())
    }

    pub fn text_mut_by_id<'a>(&'a mut self, id: &'a str) -> TextMut<'a> {
        if let Some((idx, _)) = self.text_parts.iter().enumerate().find(|(_i, (part_id, _))| part_id == id) {
            let text = &mut self.text_parts[idx].1;
            TextMut::occupied(text)
        } else {
            TextMut::vacant(&mut self.text_parts, id)
        }
    }
}

impl From<String> for Text {
    fn from(text: String) -> Self {
        Self {
            text_parts: vec![(String::new(), text)],
        }
    }
}

impl From<&str> for Text {
    fn from(text: &str) -> Self {
        Self {
            text_parts: vec![(String::new(), text.to_string())],
        }
    }
}

impl FromIterator<(String, String)> for Text {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
        Self {
            text_parts: iter.into_iter().collect(),
        }
    }
}

impl IntoIterator for Text {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<(String, String)>;

    fn into_iter(self) -> Self::IntoIter {
        self.text_parts.into_iter()
    }
}

#[derive(Debug)]
pub struct TextMut<'a> {
    inner: TextMutInner<'a>,
}

#[derive(Debug)]
enum TextMutInner<'a> {
    Occupied(Option<&'a mut String>),
    Vacant(&'a mut Vec<(String, String)>, &'a str, Option<&'a mut String>),
}

impl<'a> TextMut<'a> {
    fn occupied(text: &'a mut String) -> Self {
        Self {
            inner: TextMutInner::Occupied(Some(text)),
        }
    }

    fn vacant(text_parts: &'a mut Vec<(String, String)>, id: &'a str) -> Self {
        Self {
            inner: TextMutInner::Vacant(text_parts, id, None),
        }
    }

    pub fn or_insert(self, text: String) -> &'a mut String {
        match self.inner {
            TextMutInner::Occupied(Some(entry)) => {
                entry
            },
            TextMutInner::Vacant(text_parts, id, _) => {
                text_parts.push((id.to_string(), text));
                &mut text_parts.last_mut().unwrap().1
            },
            TextMutInner::Occupied(None) => {
                // TextMutInner::Occupied(None) is never constructed
                unreachable!();
            },
        }
    }
}

impl<'a> Deref for TextMut<'a> {
    type Target = Option<&'a mut String>;

    fn deref(&self) -> &Option<&'a mut String> {
        match &self.inner {
            TextMutInner::Occupied(entry) => entry,
            TextMutInner::Vacant(_, _, entry) => entry,
        }
    }
}

impl<'a> DerefMut for TextMut<'a> {
    fn deref_mut(&mut self) -> &mut Option<&'a mut String> {
        match &mut self.inner {
            TextMutInner::Occupied(entry) => entry,
            TextMutInner::Vacant(_, _, entry) => entry,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text() {
        let text = Text::from("hello");
        assert_eq!(text.text(), Some("hello"));

        let text: Text = [("a".into(), "hello".into())].into_iter().collect();
        assert_eq!(text.text(), Some("hello"));

        let text: Text = [("a".into(), "hello".into()), ("b".into(), "world".into())].into_iter().collect();
        assert_eq!(text.text(), Some("hello"));
    }

    #[test]
    fn text_mut() {
        let mut text = Text::from("hello");
        let mut t_mut = text.text_mut();
        let s_mut = t_mut.or_insert("world".into());
        assert_eq!(*s_mut, "hello");

        let mut text: Text = [("a".into(), "hello".into())].into_iter().collect();
        let mut t_mut = text.text_mut();
        let s_mut = t_mut.or_insert("world".into());
        assert_eq!(*s_mut, "hello");
    }

    #[test]
    fn text_by_id() {
        let text = Text::from("hello");
        assert_eq!(text.text_by_id("a"), None);

        let text: Text = [("a".into(), "hello".into())].into_iter().collect();
        assert_eq!(text.text_by_id("a"), Some("hello"));
        assert_eq!(text.text_by_id("b"), None);

        let text: Text = [("a".into(), "hello".into()), ("b".into(), "world".into())].into_iter().collect();
        assert_eq!(text.text_by_id("a"), Some("hello"));
        assert_eq!(text.text_by_id("b"), Some("world"));
        assert_eq!(text.text_by_id("c"), None);
    }

    #[test]
    fn text_mut_by_id() {
        let mut text = Text::from("hello");
        let mut t_mut = text.text_mut_by_id("a");
        let s_mut = t_mut.or_insert("world".into());
        assert_eq!(*s_mut, "world");

        let mut text: Text = [("a".into(), "hello".into())].into_iter().collect();
        let mut t_mut = text.text_mut_by_id("a");
        let s_mut = t_mut.or_insert("world".into());
        assert_eq!(*s_mut, "hello");
    
        let mut text: Text = [("a".into(), "hello".into())].into_iter().collect();
        let mut t_mut = text.text_mut_by_id("b");
        let s_mut = t_mut.or_insert("world".into());
        assert_eq!(*s_mut, "world");
        assert_eq!(text.into_iter().collect::<Vec<_>>(), vec![("a".into(), "hello".into()), ("b".into(), "world".into())]);
    }
}