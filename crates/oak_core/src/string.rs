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
    let token = if token.contains("\r\n") {
        Cow::Owned(token.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(token)
    };
    let token = token.as_ref();

    match token.chars().next()? {
        'r' | 'R' => decode_raw_string(&token[1..]),
        '"' | '\'' => decode_quoted_string(token),
        _ => None,
    }
}

fn decode_raw_string(token: &str) -> Option<String> {
    let quote = token
        .chars()
        .next()
        .filter(|quote| matches!(quote, '"' | '\''))?;
    let rest = &token[1..];

    let dashes = rest.bytes().take_while(|byte| *byte == b'-').count();
    let rest = &rest[dashes..];

    let close = match rest.chars().next()? {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    let body = &rest[1..];

    // R ends the string at the first closing bracket followed by the same
    // dashes and quote. Any text after that sequence is outside the literal.
    let terminator = format!("{close}{}{quote}", "-".repeat(dashes));
    let end = body.find(&terminator)?;
    if end + terminator.len() != body.len() {
        return None;
    }

    Some(body[..end].to_string())
}

fn decode_quoted_string(token: &str) -> Option<String> {
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

    loop {
        let c = chars.next()?;

        if c == quote {
            break;
        }

        let c = if c == '\\' {
            match chars.next()? {
                digit @ '0'..='7' => {
                    bytes.push(octal_escape(digit, &mut chars)?);
                    has_byte_escape = true;
                    continue;
                },
                'x' => {
                    bytes.push(hex_escape(&mut chars)?);
                    has_byte_escape = true;
                    continue;
                },
                'u' => {
                    code_points.push(unicode_escape(&mut chars, 4)?);
                    has_unicode_escape = true;
                    continue;
                },
                'U' => {
                    let code_point = unicode_escape(&mut chars, 8)?;
                    if code_point > 0x10FFFF {
                        return None;
                    }
                    code_points.push(code_point);
                    has_unicode_escape = true;
                    continue;
                },
                'a' => '\x07',
                'b' => '\x08',
                'f' => '\x0C',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'v' => '\x0B',
                c @ ('\\' | '"' | '\'' | '`' | ' ' | '\n') => c,
                _ => return None,
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
        return string_from_code_points(&code_points);
    }

    String::from_utf8(bytes).ok()
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
}
