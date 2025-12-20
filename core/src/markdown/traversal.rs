use std::ops::Range;

use pulldown_cmark::{CowStr, Event, HeadingLevel, OffsetIter, Parser, TagEnd};
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
    event_stack: Vec<TraversalEvent<'a>>,
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
            event_stack: Vec::new(),
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
                    let done = event == Event::End(TagEnd::Paragraph);
                    self.event_stack.push(TraversalEvent::Markdown(event));
                    if done { break; }
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
                self.event_stack.push(TraversalEvent::Markdown(event));
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
                self.event_stack.push(node_event);

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
                self.event_stack.push(TraversalEvent::XmlStart {
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
                    self.event_stack.push(TraversalEvent::RegionStart {
                        unit: unit,
                        value: value,
                        attributes: other_attrs,
                    });
                } else {
                    // Create XML empty event
                    self.event_stack.push(TraversalEvent::XmlEmpty {
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
                self.event_stack.push(end_event);
            }
            TraversalEffect::CloseNodeAndNotifyDescendants => {
                // Remove node from stack
                let target_node = self.node_stack.remove(target_node_index);

                // Notify descendants about parent closing
                for i in (target_node_index..self.node_stack.len()).rev() {
                    let descendant_effect = self.node_stack[i].on_parent_closing(&target_node);
                    self.process_effect(i, descendant_effect);
                }

                // Push end event
                let end_event = target_node.to_end();
                self.event_stack.push(end_event);
            }
            TraversalEffect::CloseNodeAndContinue => {
                // Remove node from stack and push end event
                let target_node = self.node_stack.remove(target_node_index);
                let end_event = target_node.to_end();
                self.event_stack.push(end_event);
                // Continue processing
                return true;
            }
            TraversalEffect::CloseNodeAndError(err) => {
                // Remove node from stack
                let target_node = self.node_stack.remove(target_node_index);
                // Push error event
                self.event_stack.push(TraversalEvent::Error(err));
                // Push end event
                let end_event = target_node.to_end();
                self.event_stack.push(end_event);
            }
        }
        return false;
    }
}

impl<'a> Iterator for Traverse<'a> {
    type Item = TraversalEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(event) = self.event_stack.pop() {
            return Some(event);
        }
        
        None
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

    fn on_parent_closing(&self, other: &TraversalNode<'a>) -> TraversalEffect<'a> {
        match (self, other) {
            (TraversalNode::Xml(self_name), TraversalNode::Xml(other_name)) => {
                xml_node::on_parent_closing(self_name, other_name)
            }
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
        _other_name: &'a str,
    ) -> TraversalEffect<'a> {
        TraversalEffect::CloseNodeAndError(TraverseMarkdownError::ExpectedXmlEnd(
            CowStr::from(self_name),
        ))
    }
}

mod section_node {
    use super::*;

    pub fn on_start_marker<'a>(
        self_level: HeadingLevel,
        other_level: HeadingLevel,
    ) -> TraversalEffect<'a> {
        if other_level < self_level {
            TraversalEffect::CloseNodeAndContinue
        } else if other_level == self_level {
            TraversalEffect::CloseNode
        } else {
            TraversalEffect::None
        }
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
}

enum TraversalEffect<'a> {
    None,
    CloseNode,
    CloseNodeAndNotifyDescendants,
    CloseNodeAndContinue,
    CloseNodeAndError(TraverseMarkdownError<'a>),
}

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