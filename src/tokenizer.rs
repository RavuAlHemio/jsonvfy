use std::fmt;
use std::io::BufRead;

use crate::io_util::{BufReadExt, IoResultOptionExt};


#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JsonToken {
    OpeningBracket,
    ClosingBracket,
    OpeningBrace,
    ClosingBrace,
    Colon,
    Comma,
    String(Vec<JsonChar>),
    Number(Vec<u8>),
    Null,
    False,
    True,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JsonChar {
    Byte(u8),
    EscapedQuote,
    EscapedBackslash,
    EscapedSlash,
    EscapedBackspace,
    EscapedFormFeed,
    EscapedLineFeed,
    EscapedCarriageReturn,
    EscapedTab,
    UnicodeEscape(u16),
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    UnknownEscape(u8),
    InvalidUnicodeEscape([u8; 4]),
    InvalidNumberCharacter(u8),
    InvalidBarewordBeginning(String),
    InvalidUtf8Sequence(Vec<JsonChar>),
    Utf8SequenceProducedSurrogate(u32),
    InvalidUtf16SurrogateSequence(Vec<JsonChar>),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::UnknownEscape(c) => write!(f, "unknown escape character {:?}", c),
            Self::InvalidUnicodeEscape(c) => write!(f, "invalid Unicode escape value {}{}{}{}", c[0], c[1], c[2], c[3]),
            Self::InvalidNumberCharacter(c) => write!(f, "invalid number character {:?}", c),
            Self::InvalidBarewordBeginning(s) => write!(f, "invalid bareword beginning {:?}", s),
            Self::InvalidUtf8Sequence(seq) => write!(f, "invalid UTF-8 sequence {:?}", seq),
            Self::Utf8SequenceProducedSurrogate(sur) => write!(f, "UTF-8 sequence produced surrogate 0x{:04X}", sur),
            Self::InvalidUtf16SurrogateSequence(seq) => write!(f, "invalid UTF-16 surrogate sequence {:?}", seq),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::UnknownEscape(_) => None,
            Self::InvalidUnicodeEscape(_) => None,
            Self::InvalidNumberCharacter(_) => None,
            Self::InvalidBarewordBeginning(_) => None,
            Self::InvalidUtf8Sequence(_) => None,
            Self::Utf8SequenceProducedSurrogate(_) => None,
            Self::InvalidUtf16SurrogateSequence(_) => None,
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self { Self::Io(value) }
}


fn do_skip_whitespace<R: BufRead>(mut json_reader: R) -> Result<bool, std::io::Error> {
    let peeked = json_reader.fill_buf()?;
    let peeked_len = peeked.len();
    if peeked_len == 0 {
        // EOF
        return Ok(false);
    }

    let first_non_whitespace = peeked.iter()
        .position(|&b|
            b != 0x20
            && b != 0x09
            && b != 0x0A
            && b != 0x0D
        );
    if let Some(fnw) = first_non_whitespace {
        // consume all the bytes until then
        json_reader.consume(fnw);
        Ok(false)
    } else {
        // all the bytes in the buffer are whitespace
        // we need to do this all over again
        json_reader.consume(peeked_len);
        Ok(true)
    }
}

pub(crate) fn skip_whitespace<R: BufRead>(mut json_reader: R) -> Result<(), std::io::Error> {
    let mut repeat = true;
    while repeat {
        repeat = do_skip_whitespace(&mut json_reader)?;
    }
    Ok(())
}


fn get_simple_token(peek: &[u8]) -> Option<JsonToken> {
    assert!(peek.len() > 0);
    match peek[0] {
        b'[' => Some(JsonToken::OpeningBracket),
        b']' => Some(JsonToken::ClosingBracket),
        b'{' => Some(JsonToken::OpeningBrace),
        b'}' => Some(JsonToken::ClosingBrace),
        b':' => Some(JsonToken::Colon),
        b',' => Some(JsonToken::Comma),
        _ => None,
    }
}


fn read_string<R: BufRead>(mut json_reader: R) -> Result<Vec<JsonChar>, Error> {
    // the string obviously starts with quotation marks
    let start_quote = json_reader.read_byte().unwrap_eof()?;
    assert_eq!(start_quote, b'"');

    let mut escaping = false;
    let mut string = Vec::new();
    loop {
        // read a byte
        let b = json_reader.read_byte().unwrap_eof()?;
        if escaping {
            match b {
                b'"' => string.push(JsonChar::EscapedQuote),
                b'\\' => string.push(JsonChar::EscapedBackslash),
                b'/' => string.push(JsonChar::EscapedSlash),
                b'b' => string.push(JsonChar::EscapedBackspace),
                b'f' => string.push(JsonChar::EscapedFormFeed),
                b'n' => string.push(JsonChar::EscapedLineFeed),
                b'r' => string.push(JsonChar::EscapedCarriageReturn),
                b't' => string.push(JsonChar::EscapedTab),
                b'u' => {
                    // Unicode escape
                    let mut escape_buf = [0u8; 4];
                    json_reader.read_exact(&mut escape_buf)?;

                    if !escape_buf.iter().all(|b| b.is_ascii_hexdigit()) {
                        return Err(Error::InvalidUnicodeEscape(escape_buf));
                    }

                    let escape_str = std::str::from_utf8(&escape_buf).unwrap();
                    let escape_value = u16::from_str_radix(escape_str, 16).unwrap();
                    string.push(JsonChar::UnicodeEscape(escape_value));
                },
                other => return Err(Error::UnknownEscape(other)),
            }
            escaping = false;
        } else {
            match b {
                b'"' => break,
                b'\\' => escaping = true,
                other => string.push(JsonChar::Byte(other)),
            }
        }
    }
    Ok(string)
}


fn read_number_string<R: BufRead>(mut json_reader: R) -> Result<Vec<u8>, Error> {
    enum ParserState {
        ExpectMinusOrZeroOrInitialMantissa,
        ExpectInitialMantissa,
        ExpectDotOrE,
        ExpectMantissaOrDotOrE,
        ExpectFractional,
        ExpectFractionalOrE,
        ExpectEPlusMinusOrInitialExponent,
        ExpectInitialExponent,
        ExpectExponent,
    }
    let mut state = ParserState::ExpectMinusOrZeroOrInitialMantissa;

    let mut number_buf = Vec::new();

    loop {
        match state {
            ParserState::ExpectMinusOrZeroOrInitialMantissa => {
                // in this state, a character is required
                let b = json_reader.read_byte().unwrap_eof()?;
                if b == b'-' {
                    number_buf.push(b);
                    state = ParserState::ExpectInitialMantissa;
                } else if b == b'0' {
                    // no leading zeroes => this must be followed by dot or E (or EOF)
                    number_buf.push(b);
                    state = ParserState::ExpectDotOrE;
                } else if b >= b'1' && b <= b'9' {
                    number_buf.push(b);
                    state = ParserState::ExpectMantissaOrDotOrE;
                } else {
                    return Err(Error::InvalidNumberCharacter(b));
                }
            },
            ParserState::ExpectInitialMantissa => {
                // in this state, a character is required
                let b = json_reader.read_byte().unwrap_eof()?;
                if b == b'0' {
                    // no leading zeroes => this must be followed by dot or E (or EOF)
                    number_buf.push(b);
                    state = ParserState::ExpectDotOrE;
                } else if b >= b'1' && b <= b'9' {
                    number_buf.push(b);
                    state = ParserState::ExpectMantissaOrDotOrE;
                } else {
                    return Err(Error::InvalidNumberCharacter(b));
                }
            },
            ParserState::ExpectDotOrE => {
                // in this state, a character is optional
                match json_reader.peek()? {
                    Some(b) => {
                        if b == b'.' {
                            number_buf.push(b);
                            json_reader.consume(1);
                            state = ParserState::ExpectFractional;
                        } else if b == b'E' || b == b'e' {
                            number_buf.push(b);
                            json_reader.consume(1);
                            state = ParserState::ExpectEPlusMinusOrInitialExponent;
                        } else {
                            return Ok(number_buf);
                        }
                    },
                    None => return Ok(number_buf),
                }
            },
            ParserState::ExpectMantissaOrDotOrE => {
                // in this state, a character is optional
                match json_reader.peek()? {
                    Some(b) => {
                        if b >= b'0' && b <= b'9' {
                            number_buf.push(b);
                            json_reader.consume(1);
                            state = ParserState::ExpectMantissaOrDotOrE;
                        } else if b == b'.' {
                            number_buf.push(b);
                            json_reader.consume(1);
                            state = ParserState::ExpectFractional;
                        } else if b == b'E' || b == b'e' {
                            number_buf.push(b);
                            json_reader.consume(1);
                            state = ParserState::ExpectEPlusMinusOrInitialExponent;
                        } else {
                            return Ok(number_buf);
                        }
                    },
                    None => return Ok(number_buf),
                }
            },
            ParserState::ExpectFractional => {
                // in this state, a character is required
                let b = json_reader.read_byte().unwrap_eof()?;
                if b >= b'0' && b <= b'9' {
                    number_buf.push(b);
                    state = ParserState::ExpectFractionalOrE;
                } else {
                    return Err(Error::InvalidNumberCharacter(b));
                }
            },
            ParserState::ExpectFractionalOrE => {
                // in this state, a character is optional
                match json_reader.peek()? {
                    Some(b) => {
                        if b >= b'0' && b <= b'9' {
                            number_buf.push(b);
                            json_reader.consume(1);
                            // same state
                        } else if b == b'E' || b == b'e' {
                            number_buf.push(b);
                            json_reader.consume(1);
                            state = ParserState::ExpectEPlusMinusOrInitialExponent;
                        } else {
                            return Ok(number_buf);
                        }
                    },
                    None => return Ok(number_buf),
                }
            },
            ParserState::ExpectEPlusMinusOrInitialExponent => {
                // in this state, a character is required
                let b = json_reader.read_byte().unwrap_eof()?;
                if b == b'+' || b == b'-' {
                    number_buf.push(b);
                    state = ParserState::ExpectInitialExponent;
                } else if b >= b'0' && b <= b'9' {
                    number_buf.push(b);
                    state = ParserState::ExpectExponent;
                } else {
                    return Err(Error::InvalidNumberCharacter(b));
                }
            },
            ParserState::ExpectInitialExponent => {
                // in this state, a character is required
                let b = json_reader.read_byte().unwrap_eof()?;
                if b >= b'0' && b <= b'9' {
                    number_buf.push(b);
                    state = ParserState::ExpectExponent;
                } else {
                    return Err(Error::InvalidNumberCharacter(b));
                }
            },
            ParserState::ExpectExponent => {
                // in this state, a character is optional
                match json_reader.peek()? {
                    Some(b) => {
                        if b >= b'0' && b <= b'9' {
                            number_buf.push(b);
                            json_reader.consume(1);
                            // same state
                        } else {
                            return Ok(number_buf);
                        }
                    },
                    None => return Ok(number_buf),
                }
            },
        }
    }
}


pub fn read_next_token<R: BufRead>(mut json_reader: R) -> Result<Option<JsonToken>, Error> {
    skip_whitespace(&mut json_reader)?;
    let peek = json_reader.fill_buf()?;
    if peek.len() == 0 {
        // EOF
        return Ok(None);
    }

    if let Some(simple_token) = get_simple_token(peek) {
        json_reader.consume(1);
        return Ok(Some(simple_token));
    }

    if peek[0] == b'"' {
        // a string begins!
        let string = read_string(json_reader)?;
        return Ok(Some(JsonToken::String(string)));
    }

    // a number always begins with either a minus or a decimal digit
    if peek[0] == b'-' || (peek[0] >= b'0' && peek[0] <= b'9') {
        let number = read_number_string(json_reader)?;
        return Ok(Some(JsonToken::Number(number)));
    }

    // otherwise, it must be a bareword
    // the shortest barewords are 4 characters long (true or null)
    let mut buf = [0u8; 4];
    json_reader.read_exact(&mut buf)?;
    if &buf == b"true" {
        return Ok(Some(JsonToken::True));
    } else if &buf == b"null" {
        return Ok(Some(JsonToken::Null));
    } else if &buf == b"fals" {
        let mut sub_buf = [0u8];
        json_reader.read_exact(&mut sub_buf)?;
        if sub_buf[0] == b'e' {
            return Ok(Some(JsonToken::False));
        }

        // e.g. "falsx"
        let mut bareword_begin = "fals".to_owned();
        bareword_begin.push(char::from_u32(sub_buf[0] as u32).unwrap());
        return Err(Error::InvalidBarewordBeginning(bareword_begin));
    } else {
        // some completely different bareword or sequence of symbols
        let mut bareword_begin = String::with_capacity(4);
        for b in buf {
            bareword_begin.push(char::from_u32(b as u32).unwrap());
        }
        return Err(Error::InvalidBarewordBeginning(bareword_begin));
    }
}


fn get_next_json_char_byte<'a, I: Iterator<Item = &'a JsonChar>>(previous_bytes: &[u8], iter: &mut I) -> Result<u8, Error> {
    match iter.next() {
        Some(JsonChar::Byte(b2)) if *b2 & 0b1100_0000 == 0b1000_0000 => Ok(*b2),
        Some(other) => {
            // invalid continuation of a UTF-8 sequence
            let mut sequence_chars: Vec<JsonChar> = previous_bytes.iter()
                .map(|b| JsonChar::Byte(*b))
                .collect();
            sequence_chars.push(*other);
            Err(Error::InvalidUtf8Sequence(sequence_chars))
        },
        None => {
            // UTF-8 sequence ended abruptly
            let sequence_chars: Vec<JsonChar> = previous_bytes.iter()
                .map(|b| JsonChar::Byte(*b))
                .collect();
            Err(Error::InvalidUtf8Sequence(sequence_chars))
        },
    }
}


pub fn interpret_string(json_chars: &[JsonChar]) -> Result<String, Error> {
    let mut chars = Vec::with_capacity(json_chars.len());

    let mut iter = json_chars.into_iter();
    while let Some(json_char) = iter.next() {
        match *json_char {
            JsonChar::Byte(b) => {
                // process as UTF-8
                if b & 0b1000_0000 == 0b0000_0000 {
                    // 0bbb_bbbb
                    chars.push(char::from_u32(b.into()).unwrap());
                } else if b & 0b1110_0000 == 0b1100_0000 {
                    // 110b_bbbb 10bb_bbbb
                    let b2 = get_next_json_char_byte(&[b], &mut iter)?;
                    let char_value =
                        u32::from(b & 0b0001_1111) << 6
                        | u32::from(b2 & 0b0011_1111) << 0
                    ;
                    let c = match char::from_u32(char_value) {
                        Some(c) => c,
                        None => {
                            // value represents a UTF-16 surrogate -- invalid in UTF-8
                            return Err(Error::Utf8SequenceProducedSurrogate(char_value));
                        },
                    };
                    chars.push(c);
                } else if b & 0b1111_0000 == 0b1110_0000 {
                    // 1110_bbbb 10bb_bbbb 10bb_bbbb
                    let b2 = get_next_json_char_byte(&[b], &mut iter)?;
                    let b3 = get_next_json_char_byte(&[b, b2], &mut iter)?;
                    let char_value =
                        u32::from(b & 0b0000_1111) << 12
                        | u32::from(b2 & 0b0011_1111) << 6
                        | u32::from(b3 & 0b0011_1111) << 0
                    ;
                    let c = match char::from_u32(char_value) {
                        Some(c) => c,
                        None => {
                            // value represents a UTF-16 surrogate -- invalid in UTF-8
                            return Err(Error::Utf8SequenceProducedSurrogate(char_value));
                        },
                    };
                    chars.push(c);
                } else if b & 0b1111_1000 == 0b1111_0000 {
                    // 1111_0bbb 10bb_bbbb 10bb_bbbb 10bb_bbbb
                    let b2 = get_next_json_char_byte(&[b], &mut iter)?;
                    let b3 = get_next_json_char_byte(&[b, b2], &mut iter)?;
                    let b4 = get_next_json_char_byte(&[b, b2, b3], &mut iter)?;
                    let char_value =
                        u32::from(b & 0b0000_0111) << 18
                        | u32::from(b2 & 0b0011_1111) << 12
                        | u32::from(b3 & 0b0011_1111) << 6
                        | u32::from(b4 & 0b0011_1111) << 0
                    ;
                    let c = match char::from_u32(char_value) {
                        Some(c) => c,
                        None => {
                            // value represents a UTF-16 surrogate -- invalid in UTF-8
                            return Err(Error::Utf8SequenceProducedSurrogate(char_value));
                        },
                    };
                    chars.push(c);
                } else {
                    return Err(Error::InvalidUtf8Sequence(vec![JsonChar::Byte(b)]));
                }
            },
            JsonChar::EscapedQuote => {
                chars.push('"');
            },
            JsonChar::EscapedBackslash => {
                chars.push('\\');
            },
            JsonChar::EscapedSlash => {
                chars.push('/');
            },
            JsonChar::EscapedBackspace => {
                chars.push('\u{08}');
            },
            JsonChar::EscapedFormFeed => {
                chars.push('\u{0C}');
            },
            JsonChar::EscapedLineFeed => {
                chars.push('\n');
            },
            JsonChar::EscapedCarriageReturn => {
                chars.push('\r');
            },
            JsonChar::EscapedTab => {
                chars.push('\t');
            },
            JsonChar::UnicodeEscape(u) => {
                // process as UTF-16
                if u >= 0xD800 && u <= 0xDBFF {
                    // leading surrogate; check for trailing surrogate
                    let u2 = match iter.next() {
                        Some(JsonChar::UnicodeEscape(u2)) if *u2 >= 0xDC00 && u <= 0xDFFF => *u2,
                        Some(other) => return Err(Error::InvalidUtf16SurrogateSequence(vec![JsonChar::UnicodeEscape(u), *other])),
                        None => return Err(Error::InvalidUtf16SurrogateSequence(vec![JsonChar::UnicodeEscape(u)])),
                    };
                    let char_value =
                        0x1_0000
                        + (u32::from(u - 0xD800) << 10)
                        + u32::from(u2 - 0xDC00)
                    ;
                    chars.push(char::from_u32(char_value).unwrap());
                } else if u >= 0xDC00 && u <= 0xDFFF {
                    // trailing surrogate without a leading surrogate
                    return Err(Error::InvalidUtf16SurrogateSequence(vec![JsonChar::UnicodeEscape(u)]));
                } else {
                    // non-surrogate BMP UTF-16 escape
                    chars.push(char::from_u32(u.into()).unwrap());
                }
            },
        }
    }
    Ok(String::from_iter(chars.into_iter()))
}
