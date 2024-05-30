use std::collections::BTreeSet;
use std::io::BufRead;

use crate::io_util::BufReadExt;
use crate::tokenizer::{interpret_string, JsonToken, read_next_token, skip_whitespace};


#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum JsonStackValue {
    Array(JsonArray),
    Object(JsonObject),
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct JsonArray {
    pub current_index: usize,
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct JsonObject {
    pub known_keys: BTreeSet<String>,
    pub current_key: Option<String>,
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct ParserExpects: u8 {
        const VALUE = 0x01;
        const KEY = 0x02;
        const COMMA = 0x04;
        const COLON = 0x08;
        const CLOSING_BRACKET = 0x10;
        const CLOSING_BRACE = 0x20;
    }
}


pub fn verify<R: BufRead>(mut json_reader: R) -> bool {
    let mut json_stack = Vec::new();
    let mut expects = ParserExpects::VALUE;

    loop {
        // take a token
        let tok = match read_next_token(&mut json_reader) {
            Ok(Some(t)) => t,
            Ok(None) => break,
            Err(e) => {
                eprintln!("failed to take next token: {}", e);
                return false;
            },
        };

        match &tok {
            JsonToken::String(s) => {
                let processed_string = match interpret_string(s) {
                    Ok(ps) => ps,
                    Err(e) => {
                        eprintln!("invalid string: {}", e);
                        return false;
                    },
                };

                // strings can be keys or values
                if expects.contains(ParserExpects::KEY) {
                    match json_stack.last_mut() {
                        Some(JsonStackValue::Object(obj)) => {
                            if obj.known_keys.contains(&processed_string) {
                                eprintln!("duplicate key {:?} at {:?}", processed_string, json_stack);
                                return false;
                            }
                            obj.known_keys.insert(processed_string.clone());
                            obj.current_key = Some(processed_string);
                        },
                        other => {
                            panic!("parser expects KEY but top stack value is {:?}", other);
                        },
                    }
                    expects = ParserExpects::COLON;
                } else if expects.contains(ParserExpects::VALUE) {
                    // what's next?
                    match json_stack.last() {
                        Some(JsonStackValue::Array(_)) => {
                            expects = ParserExpects::COMMA | ParserExpects::CLOSING_BRACKET;
                        },
                        Some(JsonStackValue::Object(_)) => {
                            expects = ParserExpects::COMMA | ParserExpects::CLOSING_BRACE;
                        },
                        None => {
                            // end of document
                            break;
                        },
                    }
                } else {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return false;
                }
            },
            JsonToken::Null|JsonToken::True|JsonToken::False|JsonToken::Number(_) => {
                // singular value
                if !expects.contains(ParserExpects::VALUE) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return false;
                }

                // what's next?
                match json_stack.last() {
                    Some(JsonStackValue::Array(_)) => {
                        expects = ParserExpects::COMMA | ParserExpects::CLOSING_BRACKET;
                    },
                    Some(JsonStackValue::Object(_)) => {
                        expects = ParserExpects::COMMA | ParserExpects::CLOSING_BRACE;
                    },
                    None => {
                        // end of document
                        break;
                    },
                }
            },
            JsonToken::Colon => {
                if !expects.contains(ParserExpects::COLON) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return false;
                }

                // what's next?
                match json_stack.last() {
                    Some(JsonStackValue::Object(_)) => {
                        expects = ParserExpects::VALUE;
                    },
                    other => {
                        panic!("parser expects COLON but top stack value is {:?}", other);
                    },
                }
            },
            JsonToken::Comma => {
                if !expects.contains(ParserExpects::COMMA) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return false;
                }

                // what's next?
                match json_stack.last_mut() {
                    Some(JsonStackValue::Array(arr)) => {
                        arr.current_index += 1;
                        expects = ParserExpects::VALUE;
                    },
                    Some(JsonStackValue::Object(obj)) => {
                        obj.current_key = None;
                        expects = ParserExpects::KEY;
                    },
                    other => {
                        panic!("parser expects COLON but top stack value is {:?}", other);
                    },
                }
            },
            JsonToken::OpeningBracket => {
                if !expects.contains(ParserExpects::VALUE) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return false;
                }

                json_stack.push(JsonStackValue::Array(JsonArray::default()));
                expects = ParserExpects::VALUE | ParserExpects::CLOSING_BRACKET;
            },
            JsonToken::ClosingBracket => {
                if !expects.contains(ParserExpects::CLOSING_BRACKET) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return false;
                }

                match json_stack.pop() {
                    Some(JsonStackValue::Array(_)) => {},
                    other => {
                        panic!("parser expects CLOSING_BRACKET but popped stack value is {:?}", other);
                    },
                }

                match json_stack.last() {
                    Some(JsonStackValue::Array(_)) => {
                        expects = ParserExpects::COMMA | ParserExpects::CLOSING_BRACKET;
                    },
                    Some(JsonStackValue::Object(_)) => {
                        expects = ParserExpects::COMMA | ParserExpects::CLOSING_BRACE;
                    },
                    None => {
                        // end of document
                        break;
                    },
                }
            },
            JsonToken::OpeningBrace => {
                if !expects.contains(ParserExpects::VALUE) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return false;
                }

                json_stack.push(JsonStackValue::Object(JsonObject::default()));
                expects = ParserExpects::KEY | ParserExpects::CLOSING_BRACE;
            },
            JsonToken::ClosingBrace => {
                if !expects.contains(ParserExpects::CLOSING_BRACE) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return false;
                }

                match json_stack.pop() {
                    Some(JsonStackValue::Object(_)) => {},
                    other => {
                        panic!("parser expects CLOSING_BRACE but popped stack value is {:?}", other);
                    },
                }

                match json_stack.last() {
                    Some(JsonStackValue::Array(_)) => {
                        expects = ParserExpects::COMMA | ParserExpects::CLOSING_BRACKET;
                    },
                    Some(JsonStackValue::Object(_)) => {
                        expects = ParserExpects::COMMA | ParserExpects::CLOSING_BRACE;
                    },
                    None => {
                        // end of document
                        break;
                    },
                }
            },
        }
    }

    if json_stack.len() > 0 {
        eprintln!("JSON document ends without closing: {:?}", json_stack);
        return false;
    }

    if let Err(e) = skip_whitespace(&mut json_reader) {
        eprintln!("failed to skip final whitespace: {}", e);
        return false;
    }

    match json_reader.peek() {
        Ok(Some(_)) => {
            eprintln!("trailing garbage at end of document");
            false
        },
        Ok(None) => true,
        Err(e) => {
            eprintln!("failed to check for trailing garbage: {}", e);
            false
        },
    }
}


#[cfg(test)]
mod tests {
    fn test_verify(json: &str) -> bool {
        let cursor = std::io::Cursor::new(json);
        super::verify(cursor)
    }

    #[test]
    fn test_empty() {
        assert_eq!(test_verify("{}"), true);
        assert_eq!(test_verify("[]"), true);
    }

    #[test]
    fn test_simple() {
        assert_eq!(test_verify("{\"a\":0}"), true);
        assert_eq!(test_verify("{\"a\":0,\"b\":1}"), true);
        assert_eq!(test_verify("[0,1]"), true);

        // wrong number of colons in dict
        assert_eq!(test_verify("{\"a\",0}"), false);
        assert_eq!(test_verify("{\"a\":0:1}"), false);

        // colon in list
        assert_eq!(test_verify("[\"a\":0]"), false);

        // unterminated string
        assert_eq!(test_verify("[\"a]"), false);

        // unterminated list
        assert_eq!(test_verify("[\"a\""), false);

        // bareword
        assert_eq!(test_verify("[a]"), false);
    }

    #[test]
    fn test_boxed() {
        assert_eq!(test_verify("{\"a\":{\"b\":[0,{\"c\":1}],\"d\":\"e\"}}"), true);

        // swapped brackets
        assert_eq!(test_verify("{\"a\":{\"b\":[0,{\"c\":1]},\"d\":\"e\"}}"), false);
    }

    #[test]
    fn test_duplicate_key() {
        assert_eq!(test_verify("{\"a\":0,\"a\":0}"), false);

        // consider different encoded forms equivalent
        assert_eq!(test_verify("{\"a\":0,\"\\u0061\":0}"), false);
        assert_eq!(test_verify("{\"/\":0,\"\\/\":0}"), false);
        assert_eq!(test_verify("{\"/\":0,\"\\u002F\":0}"), false);
    }

    #[test]
    fn test_trailing_garbage() {
        assert_eq!(test_verify("{}{}"), false);
        assert_eq!(test_verify("{},{}"), false);
        assert_eq!(test_verify("{}true"), false);
        assert_eq!(test_verify("{}0"), false);
    }
}
