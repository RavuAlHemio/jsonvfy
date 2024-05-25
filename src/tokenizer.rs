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

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::UnknownEscape(c) => write!(f, "unknown escape character {:?}", c),
            Self::InvalidUnicodeEscape(c) => write!(f, "invalid Unicode escape value {}{}{}{}", c[0], c[1], c[2], c[3]),
            Self::InvalidNumberCharacter(c) => write!(f, "invalid number character {:?}", c),
            Self::InvalidBarewordBeginning(s) => write!(f, "invalid bareword beginning {:?}", s),
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
