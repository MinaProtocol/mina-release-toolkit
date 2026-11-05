//! The streaming conversion in both directions.
//!
//! A mainnet response is about 700 MB, so the data goes through in chunks and
//! only a short tail stays in memory between two reads.

use crate::error::{Error, Result};
use crate::token::Token;
use std::io::{self, BufWriter, Read, Write};

/// The number of bytes to ask for in one read.
const CHUNK_LEN: usize = 1 << 20;

/// One direction of the conversion.
pub trait Transcode {
    /// Converts the start of `pending` and returns the number of bytes it
    /// consumed. The bytes that it does not consume come back in the next call
    /// with more data after them. `at_end` is true if no more data follows.
    fn convert(&mut self, pending: &[u8], at_end: bool) -> Result<usize>;

    /// Writes out all converted data.
    fn flush(&mut self) -> Result<()>;

    /// Converts all of `reader`.
    fn run<R: Read>(&mut self, mut reader: R) -> Result<()> {
        let mut chunk = vec![0; CHUNK_LEN];
        let mut pending = Vec::with_capacity(2 * CHUNK_LEN);
        loop {
            let count = match reader.read(&mut chunk) {
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            pending.extend_from_slice(&chunk[..count]);
            let at_end = count == 0;
            let consumed = self.convert(&pending, at_end)?;
            pending.drain(..consumed);
            if at_end {
                return self.flush();
            }
        }
    }
}

/// The end of the bytes that are safe to convert now. The last `tail` bytes
/// stay pending until the end of the input, because the next read can complete
/// a token or a prefix that starts there.
fn safe_end(pending: &[u8], at_end: bool, tail: usize) -> usize {
    if at_end {
        pending.len()
    } else {
        pending.len().saturating_sub(tail)
    }
}

/// Replaces each byte that is not valid UTF-8 with a [`Token`].
pub struct Encoder<W: Write> {
    writer: BufWriter<W>,
}

impl<W: Write> Encoder<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(writer),
        }
    }

    /// Writes `bytes` as tokens.
    fn write_tokens(&mut self, bytes: &[u8]) -> io::Result<()> {
        bytes
            .iter()
            .try_for_each(|&byte| Token::write(&mut self.writer, byte))
    }
}

impl<W: Write> Transcode for Encoder<W> {
    fn convert(&mut self, pending: &[u8], at_end: bool) -> Result<usize> {
        if Token::find_prefix(pending, 0).is_some() {
            return Err(Error::PrefixInInput);
        }
        let end = safe_end(pending, at_end, Token::PREFIX.len() - 1);
        let mut position = 0;
        while position < end {
            let error = match std::str::from_utf8(&pending[position..end]) {
                Ok(_) => {
                    self.writer.write_all(&pending[position..end])?;
                    return Ok(end);
                }
                Err(error) => error,
            };
            let valid_end = position + error.valid_up_to();
            self.writer.write_all(&pending[position..valid_end])?;
            position = valid_end;
            let invalid_end = match error.error_len() {
                Some(length) => position + length,
                // A character is cut at the end. A later read can complete
                // it, and at the end of the input it is invalid.
                None if at_end => end,
                None => break,
            };
            self.write_tokens(&pending[position..invalid_end])?;
            position = invalid_end;
        }
        Ok(position)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(self.writer.flush()?)
    }
}

/// Replaces each [`Token`] with its byte.
pub struct Decoder<W: Write> {
    writer: BufWriter<W>,
}

impl<W: Write> Decoder<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(writer),
        }
    }
}

impl<W: Write> Transcode for Decoder<W> {
    fn convert(&mut self, pending: &[u8], at_end: bool) -> Result<usize> {
        // A token that starts before this end is complete in `pending`.
        let end = safe_end(pending, at_end, Token::LEN - 1);
        let mut position = 0;
        while position < end {
            let prefix = Token::find_prefix(pending, position).filter(|&start| start < end);
            let Some(start) = prefix else {
                self.writer.write_all(&pending[position..end])?;
                return Ok(end);
            };
            match Token::parse(&pending[start..]) {
                Some(byte) => {
                    self.writer.write_all(&pending[position..start])?;
                    self.writer.write_all(&[byte])?;
                    position = start + Token::LEN;
                }
                None => {
                    self.writer.write_all(&pending[position..=start])?;
                    position = start + 1;
                }
            }
        }
        Ok(position)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(self.writer.flush()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gives a small number of bytes on each read, to test the chunk borders.
    struct Trickle<'a> {
        data: &'a [u8],
        size: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.size.min(self.data.len()).min(buffer.len());
            buffer[..count].copy_from_slice(&self.data[..count]);
            self.data = &self.data[count..];
            Ok(count)
        }
    }

    fn encoded_from<R: Read>(reader: R) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        Encoder::new(&mut output).run(reader)?;
        Ok(output)
    }

    fn decoded_from<R: Read>(reader: R) -> Vec<u8> {
        let mut output = Vec::new();
        Decoder::new(&mut output).run(reader).unwrap();
        output
    }

    fn encoded(data: &[u8]) -> Result<Vec<u8>> {
        encoded_from(data)
    }

    fn decoded(data: &[u8]) -> Vec<u8> {
        decoded_from(data)
    }

    #[test]
    fn valid_utf8_is_not_changed() {
        let input = "{\"token_symbol\":\"żółć€😀\"}".as_bytes();
        assert_eq!(encoded(input).unwrap(), input);
        assert_eq!(decoded(input), input);
    }

    #[test]
    fn invalid_byte_becomes_a_token() {
        assert_eq!(
            encoded(b"{\"s\":\"\xff\"}").unwrap(),
            b"{\"s\":\"__MINA_BYTE_ff__\"}"
        );
    }

    #[test]
    fn each_byte_of_a_cut_sequence_gets_a_token() {
        assert_eq!(
            encoded(b"\xe2\x82A").unwrap(),
            b"__MINA_BYTE_e2____MINA_BYTE_82__A"
        );
        assert_eq!(
            encoded(b"\xe2\x82").unwrap(),
            b"__MINA_BYTE_e2____MINA_BYTE_82__"
        );
    }

    #[test]
    fn utf8_form_of_a_surrogate_is_invalid() {
        let output = encoded(b"\xed\xa0\x80").unwrap();
        assert_eq!(decoded(&output), b"\xed\xa0\x80");
        assert!(std::str::from_utf8(&output).is_ok());
    }

    #[test]
    fn encoder_rejects_the_prefix_in_the_input() {
        let error = encoded(b"a __MINA_BYTE_ff__ b").unwrap_err();
        assert!(matches!(error, Error::PrefixInInput));
        // Also without a complete token: it can join with a real token.
        assert!(encoded(b"__MINA_BYTE_ff\xff").is_err());
    }

    #[test]
    fn encoder_finds_the_prefix_across_a_chunk_border() {
        for size in 1..=Token::PREFIX.len() + 1 {
            let reader = Trickle {
                data: b"abc__MINA_BYTE_def",
                size,
            };
            assert!(encoded_from(reader).is_err(), "read size {size}");
        }
    }

    #[test]
    fn decoder_leaves_text_that_is_not_a_token() {
        for text in [
            &b"__MINA_BYTE_zz__"[..],
            b"__MINA_BYTE_ff_",
            b"__MINA_BYTE_",
        ] {
            assert_eq!(decoded(text), text);
        }
    }

    #[test]
    fn prefix_without_final_underscore_next_to_a_token() {
        let input = b"__MINA_BYTE\xff";
        assert_eq!(decoded(&encoded(input).unwrap()), input);
    }

    #[test]
    fn long_runs_cross_the_chunk_size() {
        let mut input = vec![b'a'; CHUNK_LEN - 1];
        input.extend_from_slice("€".as_bytes());
        input.push(0xff);
        input.extend(vec![b'b'; CHUNK_LEN]);
        let output = encoded(&input).unwrap();
        assert_eq!(output.len(), input.len() - 1 + Token::LEN);
        assert_eq!(decoded(&output), input);
    }

    /// Bytes picked to make borders and near-tokens common.
    #[test]
    fn round_trip_of_generated_input_at_all_read_sizes() {
        let parts: &[&[u8]] = &[
            b"_",
            b"__",
            b"__MINA",
            b"__MINA_BYTE",
            b"MINA_BYTE_",
            b"ff",
            b"f",
            b"\xff",
            b"\xc3",
            b"\xe2\x82",
            b"\xac",
            "€".as_bytes(),
            b"\"",
            b"a",
        ];
        // xorshift, so that a failure repeats.
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize
        };
        for _ in 0..3000 {
            let mut input = Vec::new();
            for _ in 0..next() % 12 {
                input.extend_from_slice(parts[next() % parts.len()]);
            }
            let Ok(output) = encoded(&input) else {
                assert!(Token::find_prefix(&input, 0).is_some());
                continue;
            };
            assert!(std::str::from_utf8(&output).is_ok());
            assert_eq!(decoded(&output), input, "input {input:?}");
            for size in [1, 2, 3, 7] {
                let reader = Trickle { data: &input, size };
                assert_eq!(encoded_from(reader).unwrap(), output, "read size {size}");
                let reader = Trickle {
                    data: &output,
                    size,
                };
                assert_eq!(decoded_from(reader), input, "read size {size}");
            }
        }
    }
}
