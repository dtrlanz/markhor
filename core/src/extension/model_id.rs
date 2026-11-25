#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId {
    name: Box<str>,
    provider: Box<str>,
}

impl ModelId {
    pub fn new(name: &str, provider: &str) -> Self {
        Self {
            name: name.into(),
            provider: provider.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }
}
