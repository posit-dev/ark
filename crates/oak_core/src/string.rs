use std::borrow::Cow;
use std::iter::Peekable;
use std::str::Chars;

/// Decode an R string literal, including its quotes, into its value.
///
/// Follows R's lexer, `StringValue()` and `RawStringValue()` in
/// `src/main/gram.y`.
///
/// - Quoted strings use `"` or `'`. The escapes are `\a`, `\b`, `\f`, `\n`,
///   `\r`, `\t`, `\v`, `\\`, `\"`, `\'`, `` \` ``, `\ ` (a space), a
///   backslash before a line break, one to three octal digits up to `\377`,
///   `\x` with one or two hex digits, `\u` with one to four hex digits and
///   `\U` with one to eight, the last two optionally in braces (`\u{e9}`).
///   A line break written in the string is kept.
/// - Raw strings are `r"(...)"`, with `r` or `R`, either quote, `(`, `[` or
///   `{`, and any number of dashes between the quote and the bracket
///   (`r"-(...)-"`). Escapes are not interpreted.
///
/// A `\r\n` line break counts as `\n`, including after a backslash, as when R
/// reads a file with CRLF line endings through `readLines()` (`source()`, or
/// `parse(file = )`). R's lexer alone would reject a backslash before `\r`.
///
/// Returns `None` for malformed literals, including an unrecognized escape
/// such as `\R`, `\x`, `\u` or `\U` without hex digits, a brace left open, a
/// NUL escape, an octal value above `\377`, a `\U` value above
/// `0x10FFFF`, or `\u` or `\U` escapes in the same string as octal or `\x`
/// escapes.
///
/// Also returns `None` when R accepts the literal but its value isn't valid
/// UTF-8. Octal and `\x` escapes can produce any byte, and `\u` and `\U` can
/// produce unpaired surrogates. As in R, a high surrogate escape directly
/// followed by a low surrogate escape gives one character
/// (`"\uD83D\uDE00"`).
pub fn decode_string(token: &str) -> Option<String> {
    decode(token, Extent::Token)
}

/// Decode an R string literal's text up to the cursor into its value.
/// Include the opening quote, or the `r` or `R` prefix of a raw string.
///
/// Uses the rules of [`decode_string()`], but accepts a missing closing
/// quote, an unfinished raw-string opening such as `r"--`, or an escape
/// ending at the cursor. An unfinished raw-string opening gives an empty
/// value. A trailing escape such as `\` or `\u{4` is omitted, even if it
/// cannot become valid as typing continues. A trailing high surrogate
/// (`\uD83D`) or incomplete UTF-8 byte sequence (`\xc3`) is also omitted.
///
/// A closing quote must be the prefix's last character. Text after it gives
/// `None`. With a closing quote, decoding is as strict as [`decode_string()`],
/// so an incomplete character before the quote gives `None`.
///
/// Malformed text before the end of the prefix gives `None`.
pub fn decode_string_prefix(prefix: &str) -> Option<String> {
    decode(prefix, Extent::Prefix)
}

/// Whether the decoded text is a whole literal or may stop before its end.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Extent {
    Token,
    Prefix,
}

fn decode(token: &str, extent: Extent) -> Option<String> {
    let token = if token.contains("\r\n") {
        Cow::Owned(token.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(token)
    };
    let token = token.as_ref();

    match token.chars().next()? {
        'r' | 'R' => decode_raw_string(&token[1..], extent),
        '"' | '\'' => decode_quoted_string(token, extent),
        _ => None,
    }
}

fn decode_raw_string(token: &str, extent: Extent) -> Option<String> {
    let quote = token
        .chars()
        .next()
        .filter(|quote| matches!(quote, '"' | '\''))?;
    let rest = &token[1..];

    let dashes = rest.bytes().take_while(|byte| *byte == b'-').count();
    let rest = &rest[dashes..];

    let close = match rest.chars().next() {
        Some('(') => ')',
        Some('[') => ']',
        Some('{') => '}',
        None if extent == Extent::Prefix => return Some(String::new()),
        _ => return None,
    };
    let body = &rest[1..];

    // R ends the string at the first closing bracket followed by the same
    // dashes and quote. Any text after that sequence is outside the literal.
    let terminator = format!("{close}{}{quote}", "-".repeat(dashes));
    let Some(end) = body.find(&terminator) else {
        return match extent {
            Extent::Token => None,
            Extent::Prefix => Some(body.to_string()),
        };
    };
    if end + terminator.len() != body.len() {
        return None;
    }

    Some(body[..end].to_string())
}

fn decode_quoted_string(token: &str, extent: Extent) -> Option<String> {
    let mut chars = token.chars().peekable();
    let quote = chars.next()?;

    // Octal and `\x` escapes produce bytes, while `\u` and `\U` produce
    // code points that can include surrogate pairs. Keep literal characters
    // in both buffers until the escape kind determines which one to return.
    // R rejects strings that mix byte and Unicode escapes.
    let mut bytes: Vec<u8> = Vec::new();
    let mut code_points: Vec<u32> = Vec::new();
    let mut has_byte_escape = false;
    let mut has_unicode_escape = false;
    // An open prefix may end in a character that later escapes complete.
    let mut open = false;

    loop {
        let Some(c) = chars.next() else {
            match extent {
                Extent::Token => return None,
                Extent::Prefix => {
                    open = true;
                    break;
                },
            }
        };

        if c == quote {
            break;
        }

        let c = if c == '\\' {
            // A failed escape has not appended any output, so omitting it
            // preserves the value decoded before the backslash.
            let Some(escape) = escape(&mut chars) else {
                if extent == Extent::Prefix && chars.peek().is_none() {
                    open = true;
                    break;
                }
                return None;
            };
            match escape {
                Escape::Byte(byte) => {
                    bytes.push(byte);
                    has_byte_escape = true;
                    continue;
                },
                Escape::CodePoint(code_point) => {
                    code_points.push(code_point);
                    has_unicode_escape = true;
                    continue;
                },
                Escape::Char(c) => c,
            }
        } else {
            c
        };

        let mut utf8 = [0; 4];
        bytes.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
        code_points.push(c as u32);
    }

    if chars.next().is_some() {
        return None;
    }

    if has_unicode_escape {
        if has_byte_escape {
            return None;
        }
        // A trailing high surrogate may be completed by a low-surrogate escape.
        if open && matches!(code_points.last(), Some(0xD800..=0xDBFF)) {
            code_points.pop();
        }
        return string_from_code_points(&code_points);
    }

    match String::from_utf8(bytes) {
        Ok(value) => Some(value),
        // A byte sequence cut off at the end of a prefix, as in `"\xc3`, may
        // still be completed by the next escape. `error_len()` is `None` only
        // for such a sequence.
        Err(err) if open && err.utf8_error().error_len().is_none() => {
            let valid = err.utf8_error().valid_up_to();
            let mut bytes = err.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).ok()
        },
        Err(_) => None,
    }
}

enum Escape {
    Byte(u8),
    CodePoint(u32),
    Char(char),
}

/// Decode the escape after a backslash. Return `None` if it is malformed
/// or incomplete.
fn escape(chars: &mut Peekable<Chars>) -> Option<Escape> {
    let escape = match chars.next()? {
        digit @ '0'..='7' => Escape::Byte(octal_escape(digit, chars)?),
        'x' => Escape::Byte(hex_escape(chars)?),
        'u' => Escape::CodePoint(unicode_escape(chars, 4)?),
        'U' => {
            let code_point = unicode_escape(chars, 8)?;
            if code_point > 0x10FFFF {
                return None;
            }
            Escape::CodePoint(code_point)
        },
        'a' => Escape::Char('\x07'),
        'b' => Escape::Char('\x08'),
        'f' => Escape::Char('\x0C'),
        'n' => Escape::Char('\n'),
        'r' => Escape::Char('\r'),
        't' => Escape::Char('\t'),
        'v' => Escape::Char('\x0B'),
        c @ ('\\' | '"' | '\'' | '`' | ' ' | '\n') => Escape::Char(c),
        _ => return None,
    };
    Some(escape)
}

fn octal_escape(first: char, chars: &mut Peekable<Chars>) -> Option<u8> {
    let mut value = first.to_digit(8)?;
    for _ in 0..2 {
        let Some(digit) = chars.peek().and_then(|c| c.to_digit(8)) else {
            break;
        };
        chars.next();
        value = value * 8 + digit;
    }

    if value == 0 {
        return None;
    }
    u8::try_from(value).ok()
}

fn hex_escape(chars: &mut Peekable<Chars>) -> Option<u8> {
    let value = hex_digits(chars, 2)?;
    if value == 0 {
        return None;
    }
    u8::try_from(value).ok()
}

fn unicode_escape(chars: &mut Peekable<Chars>, max_digits: usize) -> Option<u32> {
    let braced = chars.next_if_eq(&'{').is_some();
    let value = hex_digits(chars, max_digits)?;
    if braced && chars.next() != Some('}') {
        return None;
    }

    if value == 0 {
        return None;
    }
    Some(value)
}

fn hex_digits(chars: &mut Peekable<Chars>, max_digits: usize) -> Option<u32> {
    let mut value: Option<u32> = None;
    for _ in 0..max_digits {
        let Some(digit) = chars.peek().and_then(|c| c.to_digit(16)) else {
            break;
        };
        chars.next();
        value = Some(value.unwrap_or(0) * 16 + digit);
    }
    value
}

fn string_from_code_points(code_points: &[u32]) -> Option<String> {
    let mut out = String::with_capacity(code_points.len());
    let mut code_points = code_points.iter().copied().peekable();

    while let Some(code_point) = code_points.next() {
        let code_point = match code_point {
            0xD800..=0xDBFF => {
                let low = code_points.next_if(|low| (0xDC00..=0xDFFF).contains(low))?;
                0x10000 + ((code_point - 0xD800) << 10) + (low - 0xDC00)
            },
            _ => code_point,
        };
        out.push(char::from_u32(code_point)?);
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use crate::string::decode_string;
    use crate::string::decode_string_prefix;

    fn some(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    #[test]
    fn test_plain_strings() {
        assert_eq!(decode_string(r#""abc""#), some("abc"));
        assert_eq!(decode_string("'abc'"), some("abc"));
        assert_eq!(decode_string(r#""""#), some(""));
        assert_eq!(decode_string(r#"'a"b'"#), some("a\"b"));
        assert_eq!(decode_string("\"\u{e9}t\u{e9}\""), some("\u{e9}t\u{e9}"));
        assert_eq!(decode_string("\"a\nb\""), some("a\nb"));
    }

    #[test]
    fn test_simple_escapes() {
        assert_eq!(decode_string(r#""a\nb""#), some("a\nb"));
        assert_eq!(
            decode_string(r#""\a\b\f\n\r\t\v""#),
            some("\x07\x08\x0C\n\r\t\x0B")
        );
        assert_eq!(decode_string(r#""\\""#), some("\\"));
        assert_eq!(decode_string(r#""\"""#), some("\""));
        assert_eq!(decode_string(r#"'\''"#), some("'"));
        assert_eq!(decode_string(r#""\'\"\`""#), some("'\"`"));
        assert_eq!(decode_string(r#""\ ""#), some(" "));
        assert_eq!(decode_string("\"a\\\nb\""), some("a\nb"));
        assert_eq!(decode_string(r#""C:\\Users\\x""#), some(r"C:\Users\x"));
    }

    #[test]
    fn test_unrecognized_escapes() {
        // https://github.com/posit-dev/positron/issues/6584
        assert_eq!(decode_string(r#"".\R\utils.R""#), None);
        assert_eq!(decode_string(r#""\q""#), None);
        assert_eq!(decode_string(r#""\8""#), None);
        // Unlike CRLF, a lone carriage return after a backslash is rejected.
        assert_eq!(decode_string("\"a\\\rb\""), None);
    }

    #[test]
    fn test_crlf_line_breaks() {
        assert_eq!(decode_string("\"a\r\nb\""), some("a\nb"));
        assert_eq!(decode_string("\"a\\\r\nb\""), some("a\nb"));
        assert_eq!(decode_string("r\"(a\r\nb)\""), some("a\nb"));
        assert_eq!(decode_string("\"a\rb\""), some("a\rb"));
    }

    #[test]
    fn test_octal_escapes() {
        assert_eq!(decode_string(r#""\101""#), some("A"));
        assert_eq!(decode_string(r#""\12a""#), some("\na"));
        assert_eq!(decode_string(r#""\1234""#), some("S4"));
        assert_eq!(decode_string(r#""\378""#), some("\x1F8"));
        assert_eq!(decode_string(r#""\0""#), None);
        assert_eq!(decode_string(r#""\00""#), None);
        assert_eq!(decode_string(r#""\400""#), None);
        assert_eq!(decode_string(r#""\777""#), None);
        // This byte is not valid UTF-8 on its own.
        assert_eq!(decode_string(r#""\377""#), None);
    }

    #[test]
    fn test_hex_escapes() {
        assert_eq!(decode_string(r#""\x41""#), some("A"));
        assert_eq!(decode_string(r#""\x41g""#), some("Ag"));
        assert_eq!(decode_string(r#""\x4142""#), some("A42"));
        assert_eq!(decode_string(r#""\xa""#), some("\n"));
        assert_eq!(decode_string(r#""\xg""#), None);
        assert_eq!(decode_string(r#""\x""#), None);
        assert_eq!(decode_string(r#""\x0""#), None);
        assert_eq!(decode_string(r#""\xff""#), None);
        // Together these bytes encode U+00E9 in UTF-8.
        assert_eq!(decode_string(r#""\xc3\xa9""#), some("\u{e9}"));
        assert_eq!(decode_string("\"\u{e9}\\xe9\""), None);
    }

    #[test]
    fn test_unicode_escapes() {
        assert_eq!(decode_string(r#""\u41""#), some("A"));
        assert_eq!(decode_string(r#""\u00e9""#), some("\u{e9}"));
        assert_eq!(decode_string(r#""\u12345""#), some("\u{1234}5"));
        assert_eq!(decode_string(r#""\u{41}""#), some("A"));
        assert_eq!(decode_string(r#""\u{0041}""#), some("A"));
        assert_eq!(decode_string(r#""\u{12}3""#), some("\u{12}3"));
        assert_eq!(decode_string(r#""\u{00041}""#), None);
        assert_eq!(decode_string(r#""\u{1F600}""#), None);
        assert_eq!(decode_string(r#""\u{41""#), None);
        assert_eq!(decode_string(r#""\u{}""#), None);
        assert_eq!(decode_string(r#""\u""#), None);
        assert_eq!(decode_string(r#""\u{0}""#), None);
        assert_eq!(decode_string(r#""\u000""#), None);
    }

    #[test]
    fn test_long_unicode_escapes() {
        assert_eq!(decode_string(r#""\U1F600""#), some("\u{1F600}"));
        assert_eq!(decode_string(r#""\U{1F600}""#), some("\u{1F600}"));
        assert_eq!(decode_string(r#""\U{0001F600}""#), some("\u{1F600}"));
        assert_eq!(decode_string(r#""\U0010FFFF""#), some("\u{10FFFF}"));
        assert_eq!(decode_string(r#""\U{41}""#), some("A"));
        assert_eq!(decode_string(r#""\U{00001F600}""#), None);
        assert_eq!(decode_string(r#""\U110000""#), None);
        assert_eq!(decode_string(r#""\U{110000}""#), None);
        assert_eq!(decode_string(r#""\UFFFFFFFF""#), None);
        assert_eq!(decode_string(r#""\U0""#), None);
    }

    #[test]
    fn test_surrogates() {
        assert_eq!(decode_string(r#""\uD83D\uDE00""#), some("\u{1F600}"));
        assert_eq!(decode_string(r#""\uD83D\U0000DE00""#), some("\u{1F600}"));
        assert_eq!(decode_string(r#""\U{D83D}\U{DE00}""#), some("\u{1F600}"));
        assert_eq!(decode_string(r#""\uD800""#), None);
        assert_eq!(decode_string(r#""\uDC00x""#), None);
        assert_eq!(decode_string(r#""\uD83Dx\uDE00""#), None);
        assert_eq!(decode_string(r#""\uDE00\uD83D""#), None);
    }

    #[test]
    fn test_mixed_escapes() {
        assert_eq!(decode_string(r#""\x41\u0041""#), None);
        assert_eq!(decode_string(r#""\u00e9\x41""#), None);
        assert_eq!(decode_string(r#""\101\u0041""#), None);
        // Literal characters can accompany either byte or Unicode escapes.
        assert_eq!(decode_string("\"\u{e9}\\u00e9\""), some("\u{e9}\u{e9}"));
        assert_eq!(decode_string("\"\u{e9}\\x41\""), some("\u{e9}A"));
    }

    #[test]
    fn test_raw_strings() {
        assert_eq!(decode_string(r#"r"(a\b)""#), some(r"a\b"));
        assert_eq!(decode_string(r#"R"[x]""#), some("x"));
        assert_eq!(decode_string(r#"r"{x}""#), some("x"));
        assert_eq!(decode_string("r'(x)'"), some("x"));
        assert_eq!(decode_string(r#"r"()""#), some(""));
        assert_eq!(decode_string(r#"r"(a)b)""#), some("a)b"));
        assert_eq!(decode_string(r#"r"---(a)"--)---""#), some("a)\"--"));
        assert_eq!(decode_string(r#"r"-(a)-)-""#), some("a)-"));
        assert_eq!(decode_string("r\"(a\nb)\""), some("a\nb"));
        assert_eq!(decode_string(r#"r"(\R\utils.R)""#), some(r"\R\utils.R"));
    }

    #[test]
    fn test_malformed_tokens() {
        assert_eq!(decode_string(""), None);
        assert_eq!(decode_string("abc"), None);
        assert_eq!(decode_string(r#""abc"#), None);
        assert_eq!(decode_string(r#""a"b""#), None);
        assert_eq!(decode_string(r#""a\""#), None);
        assert_eq!(decode_string(r#"r"(x]""#), None);
        assert_eq!(decode_string(r#"r"-(x)""#), None);
        assert_eq!(decode_string(r#"r"-(a)--"-""#), None);
        assert_eq!(decode_string(r#"r"""#), None);
        assert_eq!(decode_string(r#"r"x""#), None);
        assert_eq!(decode_string("r"), None);
    }

    #[test]
    fn test_prefix_without_closing_quote() {
        assert_eq!(decode_string_prefix(r#""dat"#), some("dat"));
        assert_eq!(decode_string_prefix("'R/"), some("R/"));
        assert_eq!(decode_string_prefix(r#"""#), some(""));
        assert_eq!(decode_string_prefix(r#"'a"b"#), some("a\"b"));
        assert_eq!(decode_string_prefix(r#""a\"b"#), some("a\"b"));
        assert_eq!(decode_string_prefix("\"a\r\nb"), some("a\nb"));
    }

    #[test]
    fn test_prefix_with_closing_quote() {
        assert_eq!(decode_string_prefix(r#""dat""#), some("dat"));
        assert_eq!(decode_string_prefix(r#""""#), some(""));
        assert_eq!(decode_string_prefix(r#""a"b"#), None);
    }

    #[test]
    fn test_prefix_decodes_escapes() {
        assert_eq!(decode_string_prefix(r#""a\nb"#), some("a\nb"));
        assert_eq!(decode_string_prefix(r#""\x52/"#), some("R/"));
        assert_eq!(decode_string_prefix(r#""sub\u002fR"#), some("sub/R"));
        assert_eq!(decode_string_prefix(r#""C:\\Users\\"#), some(r"C:\Users\"));
    }

    #[test]
    fn test_prefix_ending_inside_escape() {
        assert_eq!(decode_string_prefix(r#""dat\"#), some("dat"));
        assert_eq!(decode_string_prefix(r#""C:\\Users\"#), some(r"C:\Users"));
        assert_eq!(decode_string_prefix(r#""a\x"#), some("a"));
        assert_eq!(decode_string_prefix(r#""a\u{4"#), some("a"));
        assert_eq!(decode_string_prefix(r#""a\u{"#), some("a"));
        assert_eq!(decode_string_prefix(r#""a\U"#), some("a"));
        assert_eq!(decode_string_prefix(r#""a\0"#), some("a"));
        // An escape that is complete at the end of the prefix is kept, even
        // if more digits could follow.
        assert_eq!(decode_string_prefix(r#""a\x4"#), some("a\x04"));
        assert_eq!(decode_string_prefix(r#""a\u41"#), some("aA"));
        // An incomplete byte sequence or surrogate pair is omitted.
        assert_eq!(decode_string_prefix(r#""a\xc3"#), some("a"));
        assert_eq!(decode_string_prefix(r#""a\xc3\xa9"#), some("a\u{e9}"));
        assert_eq!(decode_string_prefix(r#""a\uD83D"#), some("a"));
        assert_eq!(
            decode_string_prefix(r#""a\uD83D\uDE00"#),
            some("a\u{1F600}")
        );
    }

    #[test]
    fn test_prefix_closed_after_half_character() {
        // A closing quote prevents later escapes from completing the character.
        assert_eq!(decode_string_prefix(r#""a\xc3"#), some("a"));
        assert_eq!(decode_string_prefix(r#""a\xc3""#), None);
        assert_eq!(decode_string_prefix(r#""a\uD800"#), some("a"));
        assert_eq!(decode_string_prefix(r#""a\uD800""#), None);
    }

    #[test]
    fn test_prefix_with_malformed_escape_before_end() {
        // https://github.com/posit-dev/positron/issues/6584
        assert_eq!(decode_string_prefix(r#"".\R\ut"#), None);
        assert_eq!(decode_string_prefix(r#""\qa"#), None);
        assert_eq!(decode_string_prefix(r#""\x41\u0041"#), None);
        assert_eq!(decode_string_prefix(r#""\uD800a"#), None);
    }

    #[test]
    fn test_prefix_raw_strings() {
        assert_eq!(decode_string_prefix(r#"r"(foo"#), some("foo"));
        assert_eq!(decode_string_prefix(r#"R'[a\b"#), some(r"a\b"));
        assert_eq!(decode_string_prefix(r#"r"--{a)"-"#), some("a)\"-"));
        // A raw-string terminator needs both the closing bracket and the quote.
        assert_eq!(decode_string_prefix(r#"r"(foo)"#), some("foo)"));
        assert_eq!(decode_string_prefix(r#"r"(foo)""#), some("foo"));
        assert_eq!(decode_string_prefix(r#"r"(foo)"x"#), None);
        assert_eq!(decode_string_prefix(r#"r"("#), some(""));
        assert_eq!(decode_string_prefix(r#"r"--"#), some(""));
        assert_eq!(decode_string_prefix(r#"r""#), some(""));
        assert_eq!(decode_string_prefix(r#"r"x"#), None);
        assert_eq!(decode_string_prefix("r"), None);
    }

    #[test]
    fn test_prefix_malformed_starts() {
        assert_eq!(decode_string_prefix(""), None);
        assert_eq!(decode_string_prefix("abc"), None);
    }
}
