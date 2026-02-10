use std::ops::{Deref, DerefMut};

pub enum Content2 {
    Blob(Blob),
    Text(Vec<(String, String)>),
}

impl From<Blob> for Content2 {
    fn from(value: Blob) -> Self {
        Content2::Blob(value)
    }
}

impl<T: AsRef<str>> From<T> for Content2 {
    fn from(value: T) -> Self {
        Content2::Text(vec![(String::new(), value.as_ref().to_string())])
    }
}

impl FromIterator<(String, String)> for Content2 {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Content2::Text(iter.into_iter().collect())
    }
}

pub struct Blob {
    // TODO
}
