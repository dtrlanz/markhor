use std::{collections::VecDeque, ops::Range};

use pulldown_cmark::{CowStr, Event, HeadingLevel, OffsetIter, Parser, Tag, TagEnd};
use tracing::{debug, warn};

use crate::markdown::xml::{self, XmlTag};

pub struct TraversalCfg<'a> {
    xml_filter: Option<&'a dyn Fn(&XmlTag<'_>) -> bool>,
    enable_milestones: bool,
}

pub struct Traverse<'a> {
    content: &'a str,
    parser: OffsetIter<'a>,
    cfg: TraversalCfg<'a>,
    node_stack: Vec<TraversalNode<'a>>,
    event_queue: VecDeque<TraversalEvent<'a>>,
}

impl<'a> Traverse<'a> {
    pub fn new(content: &'a str, cfg: TraversalCfg<'a>) -> Self {
        let parser_options: pulldown_cmark::Options = [
            pulldown_cmark::Options::ENABLE_GFM,
            pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES
        ].into_iter().collect();

        let parser = pulldown_cmark::Parser::new_ext(content, parser_options).into_offset_iter();

        Traverse {
            content,
            parser,
            cfg,
            node_stack: Vec::new(),
            event_queue: VecDeque::new(),
        }
    }

    fn parse_paragraph(&mut self, range: Range<usize>, parse_exact_range: bool) {
        // If we need to parse exact range, create a new parser for the paragraph range.
        // This may be necessary if previous markdown nodes overlapped with an XML tag.
        let mut range_parser = if parse_exact_range {
            // Iterate main parser until paragraph end to ensure it resumes from the 
            // correct position afterwards.
            while let Some((event, _)) = self.parser.next() {
                if event == Event::End(TagEnd::Paragraph) {
                    break;
                }
            }

            // Create parser for paragraph range
            Some(Parser::new(&self.content[range.clone()]).into_offset_iter())
        } else {
            None
        };

        // Check if paragraph contains an XML tag
        let mut offset = 0;
        let (xml_range, xml_tag) = loop {
            // Search for next XML tag within the given range
            let parse_result = xml::parse_tag(&self.content[range.start + offset..range.end]);
            if let Some((xml_rel_range, xml_tag)) = parse_result {
                // Check if XML tag passes filter
                if self.cfg.xml_filter.as_ref().map_or(false, |filter| filter(&xml_tag)) {
                    // convert relative range to absolute range
                    let xml_range = (range.start + xml_rel_range.start)..(range.start + xml_rel_range.end);
                    break (xml_range, xml_tag);
                }
                // Continue searching for next XML tag
                offset += xml_rel_range.end;
            } else {
                // No XML tag found, push the entire paragraph as is
                // Use the appropriate parser. No need to adjust offsets here since range is not
                // used.
                let parser = range_parser.as_mut().unwrap_or(&mut self.parser);
                while let Some((event, _)) = parser.next() {
                    if event == Event::End(TagEnd::Paragraph) { 
                        break; 
                    }
                    self.event_queue.push_back(TraversalEvent::Markdown(event));
                }
                return;
            };
        };

        // Closure to get next event from appropriate parser
        let mut next_event = || if xml_range.start == range.start {
            // Paragraph begins with XML tag, skip markdown parsing
            None
        } else if let Some(parser) = range_parser.as_mut() {
            // Parser for exact range exists, we need to use it
            parser.next()
                // Adjust offsets to be absolute
                .map(|(e, r)| (e, (r.start + range.start)..(r.end + range.start)))
        } else {
            // Use main parser
            self.parser.next()
        };

        // Process markdown nodes up to the XML tag
        while let Some((event, range)) = next_event() {
            if event == Event::End(TagEnd::Paragraph) {
                break;
            }
            if range.end <= xml_range.start {
                // Entire markdown node is before the XML tag
                // Keep calm and carry on
                self.event_queue.push_back(TraversalEvent::Markdown(event));
                continue;
            } else if range.start < xml_range.start {
                // Markdown node overlaps with start of XML tag
                // Proceed depending on the node type
                let (node_event, node_end) = match event {
                    // Text nodes are clipped to the start of the XML tag
                    Event::Text(_) => {
                        let clipped_text = &self.content[range.start..xml_range.start];
                        let text_event = TraversalEvent::Markdown(Event::Text(CowStr::from(clipped_text)));
                        (text_event, xml_range.start)
                    }
                    // Inline code overrides xml parsing
                    Event::Code(code) => {
                        let code_event = TraversalEvent::Markdown(Event::Code(code));
                        (code_event, range.end)
                    }
                    // Todo: handle other markdown node types
                    // For now, convert to text and trigger warning
                    _ => {
                        warn!("Markdown node {:?} overlaps with XML tag, converting to text", event);
                        let clipped_text = &self.content[range.start..xml_range.start];
                        let text_event = TraversalEvent::Markdown(Event::Text(CowStr::from(clipped_text)));
                        (text_event, xml_range.start)
                    }
                };
                self.event_queue.push_back(node_event);

                if node_end == xml_range.start {
                    // Proceed to process the XML tag
                    break;
                }
                // Do not process XML tag invalidated by overlapping markdown node
                // Parse rest of paragraph
                self.parse_paragraph(node_end..range.end, false);
                return;
            } else if range.start == xml_range.start {
                // Previous markdown node ended where XML tag starts
                // Proceed to process the XML tag
                break;
            } else {
                // Should not happen if markdown offsets are contiguous and above logic is correct
                unreachable!(); 
            }
        }

        // Process the XML tag
        match xml_tag {
            XmlTag::Start { name, attributes } => {
                // Handle effects on node stack
                self.handle_start_marker(TraversalNode::Xml(name));
                // Create XML start event
                self.event_queue.push_back(TraversalEvent::XmlStart {
                    name: name,
                    attributes: attributes,
                });
            }
            XmlTag::End { name } => {
                // Handle effects on node stack and create XML end event
                self.handle_end_marker(TraversalNode::Xml(name));
            }
            XmlTag::Empty { name, attributes } => {
                // Try parsing as milestone
                if let Some((unit, value, other_attrs)) = self.parse_milestone(name, &attributes) {
                    // Handle effects on node stack
                    self.handle_start_marker(TraversalNode::Region(unit.clone()));

                    // Create milestone event
                    self.event_queue.push_back(TraversalEvent::RegionStart {
                        unit: unit,
                        value: value,
                        attributes: other_attrs,
                    });
                } else {
                    // Create XML empty event
                    self.event_queue.push_back(TraversalEvent::XmlEmpty {
                        name: name,
                        attributes: attributes,
                    });
                }
            }
        }

        // Parse rest of paragraph after XML tag
        self.parse_paragraph(xml_range.end..range.end, true);
    }

    fn parse_milestone(&self, name: &'a str, attributes: &Vec<(&'a str, Option<CowStr<'a>>)>) -> Option<(CowStr<'a>, CowStr<'a>, Vec<(&'a str, Option<CowStr<'a>>)>)> {
        if self.cfg.enable_milestones && name == "milestone" {
            // Extract the name and description from attributes
            let mut unit = None;
            let mut value = None;

            let attributes = attributes.iter()
                .filter_map(|(attr_name, attr_value)| {
                    if *attr_name == "unit" {
                        unit = attr_value.clone();
                        None
                    } else if *attr_name == "n" {
                        value = attr_value.clone();
                        None
                    } else {
                        Some((*attr_name, attr_value.clone()))
                    }
                }).collect();

            if let (Some(unit), Some(value)) = (unit, value) {
                return Some((unit, value, attributes));
            }
        }
        None
    }

    /// Handles the effect of an end marker on the current node stack.
    /// 
    /// Closes the affected node and handles possible effects on other open nodes. Creates all
    /// the resulting events, including the end event for the given node.
    fn handle_end_marker(&mut self, node: TraversalNode<'a>) {
        // Iterate node stack from top
        for i in (0..self.node_stack.len()).rev() {
            let stack_node = &self.node_stack[i];
            let effect = stack_node.on_end_marker(&node);
            let continue_processing = self.process_effect(i, effect);
            if !continue_processing {
                break;
            }
        }
    }

    /// Handles the effect of a start marker on the current node stack.
    /// 
    /// Adds the new node to the stack after processing possible effects on nodes that are already 
    /// open. Does not create a start event for the new node; the caller is responsible for that.
    fn handle_start_marker(&mut self, node: TraversalNode<'a>) {
        // Handle effect of start marker on open nodes
        // Iterate node stack from top (last opened node first)
        for i in (0..self.node_stack.len()).rev() {
            let stack_node = &self.node_stack[i];
            let effect = stack_node.on_start_marker(&node);
            let continue_processing = self.process_effect(i, effect);
            if !continue_processing {
                break;
            }
        }

        // Add new node to stack
        self.node_stack.push(node);
    }

    /// Processes a traversal effect on the node stack.
    ///
    /// Processes the given effect on the node at the specified index in the node stack, and 
    /// handles any resulting effects. Returns true if processing should continue to the next 
    /// node in the stack, false otherwise.
    fn process_effect(&mut self, target_node_index: usize, effect: TraversalEffect<'a>) -> bool {
        match effect {
            TraversalEffect::None => {
                // No action needed; continue processing
                return true;
            }
            TraversalEffect::CloseNode => {
                // Remove node from stack and push end event
                let target_node = self.node_stack.remove(target_node_index);
                let end_event = target_node.to_end();
                self.event_queue.push_back(end_event);
            }
            TraversalEffect::CloseNodeAndNotifyDescendants => {
                // Remove node from stack
                let target_node = self.node_stack.remove(target_node_index);

                // Notify descendants about parent closing
                for i in (target_node_index..self.node_stack.len()).rev() {
                    let descendant_effect = self.node_stack[i].on_parent_closing();
                    self.process_effect(i, descendant_effect);
                }

                // Push end event
                let end_event = target_node.to_end();
                self.event_queue.push_back(end_event);
            }
            TraversalEffect::CloseNodeAndContinue => {
                // Remove node from stack and push end event
                let target_node = self.node_stack.remove(target_node_index);
                let end_event = target_node.to_end();
                self.event_queue.push_back(end_event);
                // Continue processing
                return true;
            }
            TraversalEffect::CloseNodeAndError(err) => {
                // Remove node from stack
                let target_node = self.node_stack.remove(target_node_index);
                // Push error event
                self.event_queue.push_back(TraversalEvent::Error(err));
                // Push end event
                let end_event = target_node.to_end();
                self.event_queue.push_back(end_event);
            }
        }
        return false;
    }
}

impl<'a> Iterator for Traverse<'a> {
    type Item = TraversalEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Return queued events first
        if let Some(event) = self.event_queue.pop_front() {
            return Some(event);
        }
        
        // Process next markdown event
        if let Some((event, range)) = self.parser.next() {
            match event {
                Event::Start(Tag::Heading{ level, id, classes, attrs }) => {
                    // Handle effects on node stack
                    self.handle_start_marker(TraversalNode::Section(level));
                    // Queue section start event first, heading event later
                    // Cloning attributes etc. is a bit inelegant. But downstream consumers may
                    // need them either on the section node or on the heading event. We might 
                    // change this later if it turns out to be unnecessary in practice. 
                    // Regardless, real-world performance impact is probably negligible. 
                    self.event_queue.push_back(TraversalEvent::SectionStart {
                        level,
                        id: id.clone(),
                        classes: classes.clone(),
                        attrs: attrs.clone(),
                    });
                    // Recreate heading start event
                    self.event_queue.push_back(TraversalEvent::Markdown(Event::Start(
                        Tag::Heading { level, id, classes, attrs }
                    )));
                    // Emit event from queue. This may be a section end event if the preceding 
                    // section was closed.
                    return self.event_queue.pop_front();
                },
                Event::Start(Tag::Paragraph) => {
                    // Parse paragraph for possible XML tags
                    // This will create all relevant events between start and end of paragraph
                    self.parse_paragraph(range, false);
                    // Determine index for paragraph start event. Some paragraph-initial events 
                    // should be queued before it (e.g., xml start & empty tags).
                    let mut start_index = 0;
                    while start_index < self.event_queue.len() {
                        match self.event_queue[start_index] {
                            TraversalEvent::XmlStart { .. } => (),
                            TraversalEvent::XmlEmpty { .. } => (),
                            TraversalEvent::RegionStart { .. } => (),
                            _ => {
                                break;
                            }
                        }
                        start_index += 1;
                    }
                    // Determine index for paragraph end event. Some paragraph-final events
                    // should be queued after it (e.g., xml end tags).
                    let mut end_index = self.event_queue.len();
                    while end_index > start_index {
                        match self.event_queue[end_index - 1] {
                            TraversalEvent::XmlEnd { .. } => (),
                            TraversalEvent::RegionEnd { .. } => (),
                            _ => {
                                break;
                            }
                        }
                        end_index -= 1;
                    }
                    // Do not create empty paragraphs
                    if start_index < end_index {
                        // Trim trailing whitespace from last text node, if any
                        if let TraversalEvent::Markdown(event) = &mut self.event_queue[end_index - 1] {
                            let trimmed = if let Event::Text(text) = event {
                                Some(CowStr::from(text.trim_end()).into_static())
                            } else {
                                None
                            };
                            if let Some(trimmed) = trimmed {
                                *event = Event::Text(trimmed);
                            }
                        }
                        // Insert paragraph start event at start_index
                        self.event_queue.insert(start_index, TraversalEvent::Markdown(Event::Start(Tag::Paragraph)));
                        // Insert paragraph end event at end_index + 1 (to account for the start event)
                        self.event_queue.insert(end_index + 1, TraversalEvent::Markdown(Event::End(TagEnd::Paragraph)));
                    }
                    // Emit event from queue.
                    assert!(!self.event_queue.is_empty(), "Event queue should not be empty after parsing paragraph");
                    return self.event_queue.pop_front();
                },
                _ => {
                    // Other events are passed through as is
                    return Some(TraversalEvent::Markdown(event));
                },
            }
        }

        // Handle end of document: Close any remaining open nodes
        for i in (0..self.node_stack.len()).rev() {
            let effect = self.node_stack[i].on_parent_closing();
            self.process_effect(i, effect);
        }
        return self.event_queue.pop_front();
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum TraversalNode<'a> {
    Xml(&'a str),
    Section(HeadingLevel),
    Region(CowStr<'a>),
}

impl<'a> TraversalNode<'a> {
    fn to_end(&self) -> TraversalEvent<'a> {
        match self {
            TraversalNode::Xml(name) => TraversalEvent::XmlEnd { name },
            TraversalNode::Section(level) => TraversalEvent::SectionEnd { level: *level },
            TraversalNode::Region(unit) => TraversalEvent::RegionEnd { unit: unit.to_string().into() },
        }
    }

    fn on_start_marker(&self, other: &TraversalNode<'a>) -> TraversalEffect<'a> {
        match (self, other) {
            (TraversalNode::Xml(self_name), TraversalNode::Xml(other_name)) => {
                xml_node::on_start_marker(self_name, other_name)
            }
            (TraversalNode::Section(self_level), TraversalNode::Section(other_level)) => {
                section_node::on_start_marker(*self_level, *other_level)
            }
            (TraversalNode::Region(self_unit), TraversalNode::Region(other_unit)) => {
                region_node::on_start_marker(self_unit, other_unit)
            }
            _ => TraversalEffect::None,
        }
    }

    fn on_end_marker(&self, other: &TraversalNode<'a>) -> TraversalEffect<'a> {
        match (self, other) {
            (TraversalNode::Xml(self_name), TraversalNode::Xml(other_name)) => {
                xml_node::on_end_marker(self_name, other_name)
            }
            _ => TraversalEffect::None,
        }
    }

    fn on_parent_closing(&self) -> TraversalEffect<'a> {
        match self {
            TraversalNode::Xml(self_name) => {
                xml_node::on_parent_closing(self_name)
            },
            TraversalNode::Section(_) => {
                section_node::on_parent_closing()
            },
            TraversalNode::Region(_) => {
                region_node::on_parent_closing()
            },
            _ => TraversalEffect::None,
        }
    }
}

mod xml_node {
    use super::*;

    pub fn on_start_marker<'a>(
        _self_name: &'a str,
        _other_name: &'a str,
    ) -> TraversalEffect<'a> {
        TraversalEffect::None
    }

    pub fn on_end_marker<'a>(
        self_name: &'a str,
        other_name: &'a str,
    ) -> TraversalEffect<'a> {
        if self_name == other_name {
            TraversalEffect::CloseNodeAndNotifyDescendants
        } else {
            TraversalEffect::None
        }
    }

    pub fn on_parent_closing<'a>(
        self_name: &'a str,
    ) -> TraversalEffect<'a> {
        TraversalEffect::CloseNodeAndError(TraverseMarkdownError::ExpectedXmlEnd(
            CowStr::from(self_name),
        ))
    }
}

mod section_node {
    use super::*;

    pub fn on_start_marker(
        self_level: HeadingLevel,
        other_level: HeadingLevel,
    ) -> TraversalEffect<'static> {
        if other_level < self_level {
            TraversalEffect::CloseNodeAndContinue
        } else if other_level == self_level {
            TraversalEffect::CloseNode
        } else {
            TraversalEffect::None
        }
    }

    pub fn on_parent_closing() -> TraversalEffect<'static> {
        TraversalEffect::CloseNode
    }
}

mod region_node {
    use super::*;

    pub fn on_start_marker(
        self_unit: &str,
        other_unit: &str,
    ) -> TraversalEffect<'static> {
        if self_unit == other_unit {
            TraversalEffect::CloseNode
        } else {
            TraversalEffect::None
        }
    }

    pub fn on_parent_closing() -> TraversalEffect<'static> {
        TraversalEffect::CloseNode
    }
}

#[derive(Debug)]
enum TraversalEffect<'a> {
    None,
    CloseNode,
    CloseNodeAndNotifyDescendants,
    CloseNodeAndContinue,
    CloseNodeAndError(TraverseMarkdownError<'a>),
}

#[derive(Debug, PartialEq, Clone)]
pub enum TraversalEvent<'a> {
    XmlStart {
        name: &'a str,
        attributes: Vec<(&'a str, Option<CowStr<'a>>)>,
    },
    XmlEnd {
        name: &'a str,
    },
    XmlEmpty {
        name: &'a str,
        attributes: Vec<(&'a str, Option<CowStr<'a>>)>,
    },
    SectionStart {
        level: HeadingLevel,
        id: Option<CowStr<'a>>,
        classes: Vec<CowStr<'a>>,
        attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    },
    SectionEnd {
        level: HeadingLevel,
    },
    RegionStart {
        unit: CowStr<'a>,
        value: CowStr<'a>,
        attributes: Vec<(&'a str, Option<CowStr<'a>>)>,
    },
    RegionEnd {
        unit: CowStr<'a>,
    },
    Markdown(Event<'a>),
    Error(TraverseMarkdownError<'a>),
}

#[derive(Debug, PartialEq, Clone)]
pub enum TraverseMarkdownError<'a> {
    ExpectedXmlEnd(CowStr<'a>),
}


pub struct XmlNode<'a> {
    tag: XmlTag<'a>,
    start_offset: usize,
    end_offset: usize,
}

pub struct HeadingSection<'a> {
    level: HeadingLevel,
    id: Option<CowStr<'a>>,
    classes: Vec<CowStr<'a>>,
    attrs: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    start_offset: usize,
    end_offset: usize,
}

pub struct MilestoneSection<'a> {
    unit: CowStr<'a>,
    value: CowStr<'a>,
    start_offset: usize,
    end_offset: usize,
}


#[cfg(test)]
pub mod tests {
    use std::vec;

    use super::*;

    fn assert_events(cfg: TraversalCfg, content: &str, expected: Vec<TraversalEvent>) {
        let mut traverse = Traverse::new(content, cfg);
        let mut expected = expected;

        while let Some(actual_event) = traverse.next() {
            let expected_event = expected.remove(0);
            assert_eq!(actual_event, expected_event);
            println!("✔ {:?}", actual_event);
        }

        if !expected.is_empty() {
            panic!("Expected events remaining: {:?}", expected);
        }
    }

    fn cfg_headings_only() -> TraversalCfg<'static> {
        TraversalCfg {
            xml_filter: None,
            enable_milestones: false,
        }
    }

    fn cfg_all_xmls_tags() -> TraversalCfg<'static> {
        TraversalCfg {
            xml_filter: Some(&|_tag: &XmlTag<'_>| true),
            enable_milestones: true,
        }
    }

    #[test]
    fn nested_headings() {
        assert_events(cfg_headings_only(), 
            r#"# Heading 1

Some paragraph text.

## Heading 2

More text."#, 
            vec![
                TraversalEvent::SectionStart { level: HeadingLevel::H1, id: None, classes: vec![], attrs: vec![]},
                TraversalEvent::Markdown(Event::Start(Tag::Heading { level: HeadingLevel::H1, id: None, classes: vec![], attrs: vec![] })),
                TraversalEvent::Markdown(Event::Text(CowStr::from("Heading 1"))),
                TraversalEvent::Markdown(Event::End(TagEnd::Heading(HeadingLevel::H1))),
                TraversalEvent::Markdown(Event::Start(Tag::Paragraph)),
                TraversalEvent::Markdown(Event::Text(CowStr::from("Some paragraph text."))),
                TraversalEvent::Markdown(Event::End(TagEnd::Paragraph)),
                TraversalEvent::SectionStart { level: HeadingLevel::H2, id: None,classes: vec![], attrs: vec![] },
                TraversalEvent::Markdown(Event::Start(Tag::Heading { level: HeadingLevel::H2, id: None, classes: vec![], attrs: vec![] })),
                TraversalEvent::Markdown(Event::Text(CowStr::from("Heading 2"))),
                TraversalEvent::Markdown(Event::End(TagEnd::Heading(HeadingLevel::H2))),
                TraversalEvent::Markdown(Event::Start(Tag::Paragraph)),
                TraversalEvent::Markdown(Event::Text(CowStr::from("More text."))),
                TraversalEvent::Markdown(Event::End(TagEnd::Paragraph)),
                TraversalEvent::SectionEnd { level: HeadingLevel::H2 },
                TraversalEvent::SectionEnd { level: HeadingLevel::H1 },
            ]
        );
    }

    #[test]
    fn inverted_headings() {
        assert_events(cfg_headings_only(), 
            r#"## Heading 2

Some paragraph text.

# Heading 1

More text."#, 
            vec![
                TraversalEvent::SectionStart { level: HeadingLevel::H2, id: None, classes: vec![], attrs: vec![]},
                TraversalEvent::Markdown(Event::Start(Tag::Heading { level: HeadingLevel::H2, id: None, classes: vec![], attrs: vec![] })),
                TraversalEvent::Markdown(Event::Text(CowStr::from("Heading 2"))),
                TraversalEvent::Markdown(Event::End(TagEnd::Heading(HeadingLevel::H2))),
                TraversalEvent::Markdown(Event::Start(Tag::Paragraph)),
                TraversalEvent::Markdown(Event::Text(CowStr::from("Some paragraph text."))),
                TraversalEvent::Markdown(Event::End(TagEnd::Paragraph)),
                TraversalEvent::SectionEnd { level: HeadingLevel::H2 },
                TraversalEvent::SectionStart { level: HeadingLevel::H1, id: None,classes: vec![], attrs: vec![] },
                TraversalEvent::Markdown(Event::Start(Tag::Heading { level: HeadingLevel::H1, id: None, classes: vec![], attrs: vec![] })),
                TraversalEvent::Markdown(Event::Text(CowStr::from("Heading 1"))),
                TraversalEvent::Markdown(Event::End(TagEnd::Heading(HeadingLevel::H1))),
                TraversalEvent::Markdown(Event::Start(Tag::Paragraph)),
                TraversalEvent::Markdown(Event::Text(CowStr::from("More text."))),
                TraversalEvent::Markdown(Event::End(TagEnd::Paragraph)),
                TraversalEvent::SectionEnd { level: HeadingLevel::H1 },
            ]
        );
    }

    #[test]
    fn xml_as_separate_paragraphs() {
        assert_events(cfg_all_xmls_tags(), 
            r#"<my_tag>

Some paragraph text.

</my_tag>"#, 
            vec![
                TraversalEvent::XmlStart { name: "my_tag", attributes: vec![] },
                TraversalEvent::Markdown(Event::Start(Tag::Paragraph)),
                TraversalEvent::Markdown(Event::Text(CowStr::from("Some paragraph text."))),
                TraversalEvent::Markdown(Event::End(TagEnd::Paragraph)),
                TraversalEvent::XmlEnd { name: "my_tag" },
            ]
        );
    }

    #[test]
    fn xml_as_separate_lines() {
        assert_events(cfg_all_xmls_tags(), 
            r#"<my_tag>
Some paragraph text.
</my_tag>"#, 
            vec![
                TraversalEvent::XmlStart { name: "my_tag", attributes: vec![] },
                TraversalEvent::Markdown(Event::Start(Tag::Paragraph)),
                TraversalEvent::Markdown(Event::Text(CowStr::from("Some paragraph text."))),
                TraversalEvent::Markdown(Event::End(TagEnd::Paragraph)),
                TraversalEvent::XmlEnd { name: "my_tag" },
            ]
        );
    }

}