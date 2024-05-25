use std::collections::BTreeSet;
use std::io::BufRead;
use std::process::ExitCode;

use crate::io_util::BufReadExt;
use crate::tokenizer::{JsonChar, JsonToken, read_next_token, skip_whitespace};


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
    pub known_keys: BTreeSet<Vec<JsonChar>>,
    pub current_key: Option<Vec<JsonChar>>,
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


pub fn verify<R: BufRead>(mut json_reader: R) -> ExitCode {
    let mut json_stack = Vec::new();
    let mut expects = ParserExpects::VALUE;

    loop {
        // take a token
        let tok = match read_next_token(&mut json_reader) {
            Ok(Some(t)) => t,
            Ok(None) => break,
            Err(e) => {
                eprintln!("failed to take next token: {}", e);
                return ExitCode::FAILURE;
            },
        };

        match &tok {
            JsonToken::String(s) => {
                // strings can be keys or values
                if expects.contains(ParserExpects::KEY) {
                    match json_stack.last_mut() {
                        Some(JsonStackValue::Object(obj)) => {
                            if obj.known_keys.contains(s) {
                                eprintln!("duplicate key {:?} at {:?}", s, json_stack);
                                return ExitCode::FAILURE;
                            }
                            obj.known_keys.insert(s.clone());
                            obj.current_key = Some(s.clone());
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
                    return ExitCode::FAILURE;
                }
            },
            JsonToken::Null|JsonToken::True|JsonToken::False|JsonToken::Number(_) => {
                // singular value
                if !expects.contains(ParserExpects::VALUE) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return ExitCode::FAILURE;
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
                    return ExitCode::FAILURE;
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
                    return ExitCode::FAILURE;
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
                    return ExitCode::FAILURE;
                }

                json_stack.push(JsonStackValue::Array(JsonArray::default()));
                expects = ParserExpects::VALUE | ParserExpects::CLOSING_BRACKET;
            },
            JsonToken::ClosingBracket => {
                if !expects.contains(ParserExpects::CLOSING_BRACKET) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return ExitCode::FAILURE;
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
                    return ExitCode::FAILURE;
                }

                json_stack.push(JsonStackValue::Object(JsonObject::default()));
                expects = ParserExpects::KEY | ParserExpects::CLOSING_BRACE;
            },
            JsonToken::ClosingBrace => {
                if !expects.contains(ParserExpects::CLOSING_BRACE) {
                    eprintln!("obtained {:?}, expected {:?}", tok, expects);
                    return ExitCode::FAILURE;
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
        return ExitCode::FAILURE;
    }

    if let Err(e) = skip_whitespace(&mut json_reader) {
        eprintln!("failed to skip final whitespace: {}", e);
        return ExitCode::FAILURE;
    }

    match json_reader.peek() {
        Ok(Some(_)) => {
            eprintln!("trailing garbage at end of document");
            ExitCode::FAILURE
        },
        Ok(None) => {
            ExitCode::SUCCESS
        },
        Err(e) => {
            eprintln!("failed to check for trailing garbage: {}", e);
            ExitCode::FAILURE
        },
    }
}
