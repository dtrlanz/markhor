use std::ops::Range;

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, CowStr, HeadingLevel, LinkType, MetadataBlockKind, OffsetIter};

use super::xml::{iter_tags_with_context, TagWithContext, XmlTag};


