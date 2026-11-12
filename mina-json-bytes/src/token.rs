//! The ASCII token that stands for one invalid byte.

use std::io::{self, Write};

/// The token `__MINA_BYTE_xx__`, where `xx` is the byte in hex.
pub(crate) struct Token;

impl Token {
    /// The start of each token. Where the input holds this text, the encoder
    /// writes its first byte as a token. So the encoded text holds the prefix
    /// only at a token, or where a token touches the text around it, and
    /// `decode(encode(x)) == x`.
    pub(crate) const PREFIX: &'static str = "__MINA_BYTE_";
    const SUFFIX: &'static str = "__";
    /// Hex digits of one byte.
    const HEX_LEN: usize = 2;
    /// The length of a complete token.
    pub(crate) const LEN: usize = Self::PREFIX.len() + Self::HEX_LEN + Self::SUFFIX.len();

    /// The position of the first prefix in `haystack`, at or after `from`.
    pub(crate) fn find_prefix(haystack: &[u8], from: usize) -> Option<usize> {
        haystack[from..]
            .windows(Self::PREFIX.len())
            .position(|window| window == Self::PREFIX.as_bytes())
            .map(|index| from + index)
    }

    /// Writes the token of `byte`.
    pub(crate) fn write<W: Write>(writer: &mut W, byte: u8) -> io::Result<()> {
        write!(writer, "{}{byte:02x}{}", Self::PREFIX, Self::SUFFIX)
    }

    /// The byte of the token at the start of `bytes`, if a complete token is
    /// there.
    pub(crate) fn parse(bytes: &[u8]) -> Option<u8> {
        let rest = bytes.strip_prefix(Self::PREFIX.as_bytes())?;
        let (hex, rest) = rest.split_at_checked(Self::HEX_LEN)?;
        // from_str_radix alone also accepts a sign.
        if !rest.starts_with(Self::SUFFIX.as_bytes()) || !hex.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_parse_gives_each_byte() {
        for byte in 0..=u8::MAX {
            let mut token = Vec::new();
            Token::write(&mut token, byte).unwrap();
            assert_eq!(token.len(), Token::LEN);
            assert_eq!(Token::parse(&token), Some(byte));
        }
    }

    #[test]
    fn parse_rejects_text_that_is_not_a_token() {
        for text in [
            &b"__MINA_BYTE_zz__"[..],
            b"__MINA_BYTE_+f__",
            b"__MINA_BYTE_ff_",
            b"__MINA_BYTE_",
            b"_MINA_BYTE_ff__",
        ] {
            assert_eq!(Token::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn find_prefix_starts_at_from() {
        let text = b"a__MINA_BYTE_b__MINA_BYTE_";
        assert_eq!(Token::find_prefix(text, 0), Some(1));
        assert_eq!(Token::find_prefix(text, 2), Some(14));
        assert_eq!(Token::find_prefix(text, 15), None);
    }
}
