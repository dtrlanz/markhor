
// The enum to represent the parsed tag
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Tag<'a> {
    Start {
        name: CowStr<'a>,
        attributes: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    },
    End {
        name: CowStr<'a>,
    },
    Empty {
        name: CowStr<'a>,
        attributes: Vec<(CowStr<'a>, Option<CowStr<'a>>)>,
    },
}


/// Parses the first valid XML element tag (<tag>, </tag>, <tag/>) found in the string.
/// Skips leading text and ignores XML declarations, processing instructions, and comments.
/// Returns the range of the entire tag and the parsed Tag enum if successful.
/// Returns None if no valid tag is found or if an encountered tag is invalid.
pub fn parse_tag<'a>(s: &'a str) -> Option<(Range<usize>, Tag<'a>)> {
    let mut index = 0;

    while index < s.len() {
        // Find the next potential tag start '<'
        let tag_start_absolute = match s[index..].find('<') {
            Some(relative) => index + relative,
            None => return None, // No more '<', no tag found
        };

        // Move the outer loop index past this '<' in case parsing fails below
        index = tag_start_absolute + 1;

        // Check the first character immediately after '<'
        let mut current_pos = tag_start_absolute + 1;
        let first_char_after_lt = s[current_pos..].chars().nth(0);

        match first_char_after_lt {
            None => {
                // '<' at the very end of the string, not a valid tag
                continue; // Outer loop continues (though index is already at s.len())
            }
            Some('/') => {
                // Potential End Tag: </name>
                current_pos += 1; // Move past '/'
                current_pos = skip_whitespace(s, current_pos);

                match parse_name(s, current_pos) {
                    Some((name_range, name)) => {
                        current_pos = name_range.end;
                        current_pos = skip_whitespace(s, current_pos);

                        // Expect '>'
                        if s[current_pos..].chars().nth(0) == Some('>') {
                            let tag_end = current_pos + 1;
                            // Successfully parsed end tag, return it
                            return Some((tag_start_absolute..tag_end, Tag::End { name }));
                        } else {
                            // Invalid character after name in end tag
                            continue; // Outer loop moves to next potential '<'
                        }
                    }
                    None => {
                        // Invalid name in end tag
                        continue; // Outer loop moves to next potential '<'
                    }
                }
            }
            Some('!') => {
                 // Potential Declaration, Comment, or DTD (e.g., <!-- -->, <!DOCTYPE ...>, <![CDATA[ ... ]]>)
                 // Find the closing '>' and skip the block
                 if let Some(close_angle_relative) = s[current_pos..].find('>') {
                     index = current_pos + close_angle_relative + 1; // Set outer loop index past the closing '>'
                     continue; // Outer loop continues search from the new index
                 } else {
                     // Unclosed comment/declaration/DTD block
                     return None; // Invalid XML structure, no more valid tags possible
                 }
            }
            Some('?') => {
                // Potential Processing Instruction (e.g., <?xml ... ?>)
                // Find the closing '>' and skip the block
                 if let Some(close_angle_relative) = s[current_pos..].find("?>") {
                     index = current_pos + close_angle_relative + 2; // Set outer loop index past "?>"
                     continue; // Outer loop continues search from the new index
                 } else {
                     // Unclosed processing instruction
                     return None; // Invalid XML structure, no more valid tags possible
                 }
            }
            Some(c) if is_xml_name_start_char(c) => {
                // Potential Start or Empty Tag: <name ...> or <name .../>
                // current_pos is already after '<'
                match parse_name(s, current_pos) {
                    Some((name_range, name)) => {
                        current_pos = name_range.end;

                        match parse_attributes(s, current_pos) {
                            Some((pos_after_attrs, attributes)) => {
                                current_pos = pos_after_attrs;
                                current_pos = skip_whitespace(s, current_pos);

                                // Expect '/>' or '>'
                                if s[current_pos..].starts_with("/>") {
                                    let tag_end = current_pos + 2;
                                    // Successfully parsed empty tag
                                    return Some((tag_start_absolute..tag_end, Tag::Empty { name, attributes }));
                                } else if s[current_pos..].chars().nth(0) == Some('>') {
                                    let tag_end = current_pos + 1;
                                    // Successfully parsed start tag
                                    return Some((tag_start_absolute..tag_end, Tag::Start { name, attributes }));
                                } else {
                                    // Invalid characters after attributes
                                    continue; // Outer loop moves to next potential '<'
                                }
                            }
                            None => {
                                // Failed to parse attributes -> invalid tag
                                continue; // Outer loop moves to next potential '<'
                            }
                        }
                    }
                    None => {
                        // Invalid tag name
                        continue; // Outer loop moves to next potential '<'
                    }
                }
            }
            Some(_) => {
                // Invalid character after '<' for a tag start (e.g., '< ')
                continue; // Outer loop moves to next potential '<'
            }
        }
    }

    // Loop finished without finding a valid tag
    None
}

// --- Helper functions ---

use std::ops::Range;

use pulldown_cmark::CowStr;

/// Checks if a character is valid at the start of an XML name (simplified).
/// Allows letters, _, and :
#[inline]
fn is_xml_name_start_char(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':'
}

/// Checks if a character is valid within an XML name (simplified).
/// Allows letters, digits, ., -, _, and :
#[inline]
fn is_xml_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ':'
}

/// Skips ASCII whitespace from the given index.
/// Returns the index of the first non-whitespace character or the end of the string.
#[inline]
fn skip_whitespace(s: &str, mut index: usize) -> usize {
    while let Some(c) = s[index..].chars().next() {
        if c.is_ascii_whitespace() {
            index += c.len_utf8();
        } else {
            break;
        }
    }
    index
}

/// Parses an XML name starting at the given index.
/// Returns the range of the name and the name as CowStr if successful.
/// Assumes the starting character is valid for an XML name start.
fn parse_name<'a>(s: &'a str, start_index: usize) -> Option<(Range<usize>, CowStr<'a>)> {
    if start_index >= s.len() {
        return None;
    }

    let mut chars = s[start_index..].chars();
    let first_char = chars.next()?;
    if !is_xml_name_start_char(first_char) {
        return None; // Should not happen if called correctly after checking '<' or '</'
    }

    let mut current_pos = start_index + first_char.len_utf8();
    while let Some(c) = s[current_pos..].chars().next() {
        if is_xml_name_char(c) {
            current_pos += c.len_utf8();
        } else {
            break;
        }
    }

    if current_pos == start_index + first_char.len_utf8() && !is_xml_name_start_char(first_char) {
         // Name must contain at least one valid start character
         None
    } else if current_pos == start_index {
         // Should not happen if the first char check passed, but safety belt
         None
    }
    else {
        let name_slice = &s[start_index..current_pos];
        Some((start_index..current_pos, name_slice.into()))
    }
}

/// Resolves XML character references (&, &#xNN;, &#NN;) within a string slice.
/// Returns an owned CowStr because resolution creates new data.
fn resolve_escapes(s: &str) -> CowStr<'_> {
    // Quickly check if any potential escape sequence exists
    if !s.contains('&') {
        return s.into(); // No escapes, return owned CowStr based on original slice
    }

    let mut result_string = String::new();
    let mut current_pos = 0;

    while current_pos < s.len() {
        let rest = &s[current_pos..];
        if rest.starts_with('&') {
            // Potential entity
            if let Some(semicolon_relative_idx) = rest[1..].find(';') {
                let entity_end_absolute_idx = current_pos + 1 + semicolon_relative_idx + 1;
                let entity_slice = &s[current_pos + 1 .. entity_end_absolute_idx - 1]; // e.g. "amp" or "#x20"

                let decoded_char = match entity_slice {
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "amp" => Some('&'),
                    "apos" => Some('\''),
                    "quot" => Some('"'),
                    _ if entity_slice.starts_with('#') => {
                        let number_str = &entity_slice[1..];
                        if number_str.starts_with('x') {
                            // Hexadecimal
                            u32::from_str_radix(&number_str[1..], 16).ok().and_then(std::char::from_u32)
                        } else {
                            // Decimal
                            u32::from_str_radix(number_str, 10).ok().and_then(std::char::from_u32)
                        }
                    }
                    _ => None, // Not a recognized entity
                };

                if let Some(decoded_c) = decoded_char {
                    result_string.push(decoded_c);
                } else {
                    // Not a valid entity, treat as literal "&entity_slice;"
                    result_string.push('&');
                    result_string.push_str(entity_slice);
                    result_string.push(';');
                }
                current_pos = entity_end_absolute_idx; // Move past the entity
            } else {
                // Found '&' but no ';', treat the rest as literal
                result_string.push_str(&s[current_pos..]);
                current_pos = s.len(); // End processing
            }
        } else {
            // Not '&', copy character by character
            let c = s[current_pos..].chars().next().expect("Should be a character if current_pos < s.len()");
            result_string.push(c);
            current_pos += c.len_utf8();
        }
    }

    result_string.into() // Convert the result String to CowStr (handles Inlined/Boxed)
}


/// Parses a single attribute (name="value" or just name).
/// Returns the range of the entire attribute and the (name, Optional value) tuple.
/// Assumes the start_index is at the beginning of the attribute name.
fn parse_attribute<'a>(s: &'a str, start_index: usize) -> Option<(Range<usize>, (CowStr<'a>, Option<CowStr<'a>>))>{
    let (name_range, attr_name) = parse_name(s, start_index)?;
    let mut current_pos = name_range.end;

    current_pos = skip_whitespace(s, current_pos);

    // Check for '='
    if s[current_pos..].chars().nth(0) != Some('=') {
        // Attribute without value (e.g., <tag required>)
        // As per requirement, attribute values are optional.
        // The range is just the name's range.
        return Some((name_range, (attr_name, None)));
    }

    // Found '=', consume it
    current_pos += 1; // Move past '='
    current_pos = skip_whitespace(s, current_pos);

    // Expect a quote character
    let quote_char = match s[current_pos..].chars().nth(0) {
        Some('"') => '"',
        Some('\'') => '\'',
        _ => return None, // Invalid attribute value start
    };

    // Consume the opening quote
    let value_start_index_after_quote = current_pos + 1;
    current_pos = value_start_index_after_quote;

    // Find the closing quote
    let value_end_relative_idx = s[current_pos..].find(quote_char)?; // relative index within the rest slice
    let value_end_absolute_idx = current_pos + value_end_relative_idx;

    // The raw value slice is between the quotes
    let raw_value_slice = &s[value_start_index_after_quote..value_end_absolute_idx];

    // Resolve escapes in the value
    let attr_value = resolve_escapes(raw_value_slice);

    // The entire attribute range includes name, '=', quotes, and value
    let attribute_end_absolute_idx = value_end_absolute_idx + 1; // Move past the closing quote

    Some((start_index..attribute_end_absolute_idx, (attr_name, Some(attr_value))))
}

/// Parses zero or more attributes.
/// Returns the index after the last parsed attribute (and trailing whitespace)
/// and the vector of attributes.
/// Returns None if any attribute is invalid.
fn parse_attributes<'a>(s: &'a str, start_index: usize) -> Option<(usize, Vec<(CowStr<'a>, Option<CowStr<'a>>)>)> {
    let mut attributes = Vec::new();
    let mut current_pos = start_index;

    loop {
        current_pos = skip_whitespace(s, current_pos);

        // Check if we've reached the end of attributes ('>' or '/')
        if s[current_pos..].chars().nth(0) == Some('>') || s[current_pos..].starts_with("/>") {
            break; // End of attributes found
        }

        // Try to parse a single attribute
        match parse_attribute(s, current_pos) {
            Some((attr_range, attr)) => {
                attributes.push(attr);
                current_pos = attr_range.end; // Move position past the parsed attribute
            }
            None => {
                // Failed to parse an attribute -> invalid attribute section
                return None;
            }
        }
    }

    Some((current_pos, attributes))
}



#[cfg(test)]
mod tests {
    use pulldown_cmark::InlineStr;

    use super::*;

    // Helper to create borrowed CowStr
    fn b_cow(s: &str) -> CowStr<'_> {
        CowStr::Borrowed(s)
    }

    // Helper to create inlined CowStr
    fn i_cow(s: &str) -> CowStr<'_> {
        CowStr::Inlined(InlineStr::try_from(s).unwrap())
    }

     // Helper to create boxed CowStr
    fn x_cow(s: &str) -> CowStr<'_> {
         CowStr::Boxed(s.to_string().into_boxed_str())
    }



    #[test]
    fn test_basic_start_tag() {
        let s = "<tag>";
        let result = parse_tag(s);
        assert_eq!(result, Some((0..5, Tag::Start { name: i_cow("tag"), attributes: vec![] })));
    }

    #[test]
    fn test_basic_end_tag() {
        let s = "</tag>";
        let result = parse_tag(s);
        assert_eq!(result, Some((0..6, Tag::End { name: i_cow("tag") })));
    }

    #[test]
    fn test_basic_empty_tag() {
        let s = "<tag/>";
        let result = parse_tag(s);
        assert_eq!(result, Some((0..6, Tag::Empty { name: i_cow("tag"), attributes: vec![] })));
    }

    #[test]
    fn test_tag_with_leading_text() {
        let s = "some text <tag>";
        let result = parse_tag(s);
        assert_eq!(result, Some((10..15, Tag::Start { name: i_cow("tag"), attributes: vec![] })));
    }

     #[test]
    fn test_tag_with_trailing_text() {
        let s = "<tag> some text";
        let result = parse_tag(s);
        assert_eq!(result, Some((0..5, Tag::Start { name: i_cow("tag"), attributes: vec![] })));
    }

    #[test]
    fn test_start_tag_with_attributes() {
        let s = r#"<tag name="value" other='val2'>"#;
        let result = parse_tag(s);
        let expected_attrs = vec![
            (i_cow("name"), Some(i_cow("value"))),
            (i_cow("other"), Some(i_cow("val2"))),
        ];
        assert_eq!(result, Some((0..31, Tag::Start { name: i_cow("tag"), attributes: expected_attrs })));
    }

     #[test]
    fn test_empty_tag_with_attributes() {
        let s = r#"<tag name="value" other='val2'/>"#;
        let result = parse_tag(s);
        let expected_attrs = vec![
            (i_cow("name"), Some(i_cow("value"))),
            (i_cow("other"), Some(i_cow("val2"))),
        ];
        assert_eq!(result, Some((0..32, Tag::Empty { name: i_cow("tag"), attributes: expected_attrs })));
    }

    #[test]
    fn test_tag_with_whitespace() {
        let s = r#"  <tag   attr = "value"   />  "#;
        let result = parse_tag(s);
        let expected_attrs = vec![
            (i_cow("attr"), Some(i_cow("value"))),
        ];
        assert_eq!(result, Some((2..28, Tag::Empty { name: i_cow("tag"), attributes: expected_attrs })));
    }

    #[test]
    fn test_attribute_value_with_escapes() {
        let s = r#"<tag attr="val&amp;&lt;&gt;&apos;&quot;">"#;
        let result = parse_tag(s);
        let expected_attrs = vec![
            (i_cow("attr"), Some(CowStr::from("val&<>\'\""))),
        ];
        assert_eq!(result, Some((0..41, Tag::Start { name: i_cow("tag"), attributes: expected_attrs })));
    }

    #[test]
    fn test_attribute_without_value() {
        let s = r#"<tag required optional="false">"#;
        let result = parse_tag(s);
        let expected_attrs = vec![
            (i_cow("required"), None),
            (i_cow("optional"), Some(i_cow("false"))),
        ];
        assert_eq!(result, Some((0..31, Tag::Start { name: i_cow("tag"), attributes: expected_attrs })));
    }

    #[test]
    fn test_first_tag_only() {
        let s = "<a></a><b></b>";
        let result = parse_tag(s);
        assert_eq!(result, Some((0..3, Tag::Start { name: i_cow("a"), attributes: vec![] })));
    }

    #[test]
    fn test_ignore_comment() {
        let s = "<!-- comment --> <tag/>";
        let result = parse_tag(s);
        assert_eq!(result, Some((17..23, Tag::Empty { name: i_cow("tag"), attributes: vec![] })));
    }

    #[test]
    fn test_ignore_pi() {
        let s = "<?xml version='1.0'?> <tag/>";
        let result = parse_tag(s);
        assert_eq!(result, Some((22..28, Tag::Empty { name: i_cow("tag"), attributes: vec![] })));
    }

     #[test]
    fn test_ignore_doctype() {
        let s = "<!DOCTYPE foo> <tag/>";
        let result = parse_tag(s);
        assert_eq!(result, Some((15..21, Tag::Empty { name: i_cow("tag"), attributes: vec![] })));
    }

    #[test]
    fn test_invalid_unclosed_tag() {
        let s = "<tag";
        let result = parse_tag(s);
        assert_eq!(result, None);
    }

     #[test]
    fn test_invalid_unclosed_attribute() {
        let s = r#"<tag attr="value>"#;
        let result = parse_tag(s);
        assert_eq!(result, None); // Attribute parsing fails, tag parsing fails
    }

    #[test]
    fn test_invalid_char_after_lt() {
        let s = "< tag>";
        let result = parse_tag(s);
        assert_eq!(result, None); // '<' followed by space is not a valid tag start
    }

    #[test]
    fn test_invalid_name_start_char() {
        let s = "<1tag>";
        let result = parse_tag(s);
        assert_eq!(result, None); // '1' is not valid name start
    }

     #[test]
    fn test_invalid_char_in_name() {
        let s = "<tag!>";
        let result = parse_tag(s);
        assert_eq!(result, None); // '!' is not valid name char
    }

     #[test]
    fn test_invalid_end_tag_missing_gt() {
        let s = "</tag";
        let result = parse_tag(s);
        assert_eq!(result, None);
    }

     #[test]
    fn test_invalid_tag_missing_gt_or_slashgt() {
        let s = "<tag attr='value'";
        let result = parse_tag(s);
        assert_eq!(result, None);
    }

    #[test]
    fn test_empty_string() {
        let s = "";
        let result = parse_tag(s);
        assert_eq!(result, None);
    }

     #[test]
    fn test_whitespace_string() {
        let s = "   \n\r ";
        let result = parse_tag(s);
        assert_eq!(result, None);
    }

    #[test]
    fn test_complex_nested_like() {
        let s = "<!-- a --> text <root attr=\"val\"> <child/> </root>";
        let result = parse_tag(s);
         let expected_attrs = vec![
            (i_cow("attr"), Some(i_cow("val"))),
        ];
        // Should parse the <root> tag first
        assert_eq!(result, Some((16..33, Tag::Start { name: i_cow("root"), attributes: expected_attrs })));
    }

     #[test]
    fn test_resolve_escapes_no_escape() {
        let s = "just plain text";
        let resolved = resolve_escapes(s);
        assert_eq!(&*resolved, s);
        // This case depends on INLINE_CAPACITY, could be Inlined or Boxed/Borrowed.
        // assert!(matches!(resolved, CowStr::Borrowed(_))); // Or Inlined/Boxed
    }

     #[test]
    fn test_resolve_escapes_mixed() {
        let s = "hello & world   test < 10";
        let resolved = resolve_escapes(s);
        assert_eq!(&*resolved, "hello & world   test < 10");
    }

     #[test]
    fn test_resolve_escapes_invalid_entity() {
        let s = "value &invalid; & still literal";
        let resolved = resolve_escapes(s);
        // Invalid entity should be treated as literal
        assert_eq!(&*resolved, "value &invalid; & still literal");
    }

    #[test]
    fn test_resolve_escapes_no_semicolon() {
        let s = "value & broken";
        let resolved = resolve_escapes(s);
        assert_eq!(&*resolved, "value & broken"); // Treat as literal if no semicolon
    }

    #[test]
    fn test_tag_name_chars() {
        let s = "<a.b-c_d:e>";
        let result = parse_tag(s);
        assert_eq!(result, Some((0..11, Tag::Start { name: i_cow("a.b-c_d:e"), attributes: vec![] })));
     }

    #[test]
    fn test_attribute_name_chars() {
        let s = "<tag attr.name-ok_yes:no=\"val\"/>";
        let result = parse_tag(s);
        let expected_attrs = vec![
            (i_cow("attr.name-ok_yes:no"), Some(i_cow("val"))),
        ];
         assert_eq!(result, Some((0..32, Tag::Empty { name: i_cow("tag"), attributes: expected_attrs })));
     }

    #[test]
    fn test_long_strings_cowstr() {
        let long_name = "n".repeat(25);
        let long_value = "v".repeat(30);
        let s = format!(r#"<{0} attr="{1}">"#, long_name, long_value);

        let result = parse_tag(&s);

        match result {
            Some((range, Tag::Start { name, attributes })) => {
                assert_eq!(&*name, long_name);
                assert!(matches!(name, CowStr::Borrowed(_)), "Long name should be borrowed"); // Or Boxed if from String
                assert_eq!(attributes.len(), 1);
                let (attr_name, attr_value) = &attributes[0];
                assert_eq!(**attr_name, *"attr"); // Short name
                let value = attr_value.as_ref().unwrap();
                assert_eq!(**value, *long_value);
                assert_eq!(range.start, 0);
                assert_eq!(range.end, s.len());
            },
            _ => panic!("Parsing failed or returned unexpected type: {:?}", result),
        }
    }

    #[test]
    fn test_invalid_empty_name() {
        let s = "<>"; // Empty name
        assert_eq!(parse_tag(s), None);

        let s = "</>"; // Empty name
        assert_eq!(parse_tag(s), None);

        let s = "< />"; // Empty name after space
        assert_eq!(parse_tag(s), None);

        let s = "invalid < tag/>"; // Invalid due to whitespace before name
        assert_eq!(parse_tag(s), None);
     }

    #[test]
    fn test_invalid_attribute_syntax() {
        let s = "<tag attr=value/>"; // Missing quotes
        assert_eq!(parse_tag(s), None);

        let s = "<tag attr val/>";
        let result = parse_tag(s);
        let expected_attrs = vec![
            (i_cow("attr"), None),
            (i_cow("val"), None),
        ];
        assert_eq!(result, Some((0..15, Tag::Empty { name: i_cow("tag"), attributes: expected_attrs })));

        let s = "<tag attr = val/>"; // Missing quotes around value
        assert_eq!(parse_tag(s), None); // parse_attribute expects quotes after '='

        let s = "<tag attr = \"val />"; // Missing closing quote
        assert_eq!(parse_tag(s), None); // parse_attribute fails to find closing quote

    }
}