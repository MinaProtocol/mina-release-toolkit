//! Carry bytes that are not valid UTF-8 through a text JSON tool.
//!
//! Token symbols and zkApp URIs are arbitrary bytes, so a ledger or a runtime
//! config written by the daemon is not always valid UTF-8. jq replaces each
//! invalid byte with U+FFFD, and that changes the ledger hash.
//!
//! [`encode`] turns each invalid byte into the ASCII token `__MINA_BYTE_xx__`
//! (`xx`: the byte in lower-case hex), which a text tool passes through.
//! [`decode`] turns the tokens back into the bytes. All other bytes are copied
//! as they are.
//!
//! The prefix `__MINA_BYTE_` must not occur in the input of [`encode`]: it
//! stops with an error if it does. Without the prefix in the input, only a
//! real token can match in [`decode`], and `decode(encode(x)) == x`.
//!
//! Both functions stream, so a mainnet response of about 700 MB does not have
//! to fit in memory.

use std::io::{self, Read, Write};

pub mod jq;

const PREFIX: &[u8] = b"__MINA_BYTE_";
const SUFFIX: &[u8] = b"__";
/// Length of `__MINA_BYTE_xx__`.
const TOKEN_LEN: usize = PREFIX.len() + 2 + SUFFIX.len();
const CHUNK_LEN: usize = 1 << 20;

fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|index| from + index)
}

fn write_token<W: Write>(writer: &mut W, byte: u8) -> io::Result<()> {
    write!(writer, "__MINA_BYTE_{byte:02x}__")
}

/// Runs `step` on a buffer that holds the bytes which `step` did not consume.
/// `step` gets the buffer and true at the end of the input, and returns the
/// number of bytes it consumed.
fn stream<R: Read>(
    mut reader: R,
    mut step: impl FnMut(&[u8], bool) -> io::Result<usize>,
) -> io::Result<()> {
    let mut chunk = vec![0; CHUNK_LEN];
    let mut buffer = Vec::with_capacity(2 * CHUNK_LEN);
    loop {
        let count = match reader.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        buffer.extend_from_slice(&chunk[..count]);
        let at_end = count == 0;
        let consumed = step(&buffer, at_end)?;
        buffer.drain(..consumed);
        if at_end {
            return Ok(());
        }
    }
}

fn encode_step<W: Write>(buffer: &[u8], at_end: bool, writer: &mut W) -> io::Result<usize> {
    if find(buffer, 0, PREFIX).is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the input already contains the text __MINA_BYTE_",
        ));
    }
    // The bytes after the limit can be the start of a prefix that the next
    // read completes, so they stay in the buffer.
    let limit = if at_end {
        buffer.len()
    } else {
        buffer.len().saturating_sub(PREFIX.len() - 1)
    };
    let mut position = 0;
    while position < limit {
        match std::str::from_utf8(&buffer[position..limit]) {
            Ok(_) => {
                writer.write_all(&buffer[position..limit])?;
                position = limit;
            }
            Err(error) => {
                let valid_end = position + error.valid_up_to();
                writer.write_all(&buffer[position..valid_end])?;
                position = valid_end;
                let invalid_end = match error.error_len() {
                    Some(length) => position + length,
                    // A character is cut at the limit. A later read can
                    // complete it, and at the end of the input it is invalid.
                    None if at_end => limit,
                    None => break,
                };
                for &byte in &buffer[position..invalid_end] {
                    write_token(writer, byte)?;
                }
                position = invalid_end;
            }
        }
    }
    Ok(position)
}

/// Copies `reader` to `writer` and replaces each byte that is not valid UTF-8
/// with a token.
pub fn encode<R: Read, W: Write>(reader: R, mut writer: W) -> io::Result<()> {
    stream(reader, |buffer, at_end| {
        encode_step(buffer, at_end, &mut writer)
    })?;
    writer.flush()
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

/// The byte of the token at the start of `bytes`, if a complete token is there.
fn token_byte(bytes: &[u8]) -> Option<u8> {
    if bytes.len() < TOKEN_LEN || !bytes.starts_with(PREFIX) {
        return None;
    }
    let high = hex_value(bytes[PREFIX.len()])?;
    let low = hex_value(bytes[PREFIX.len() + 1])?;
    bytes[PREFIX.len() + 2..]
        .starts_with(SUFFIX)
        .then_some(high << 4 | low)
}

fn decode_step<W: Write>(buffer: &[u8], at_end: bool, writer: &mut W) -> io::Result<usize> {
    // A token that starts before the limit is complete in the buffer.
    let limit = if at_end {
        buffer.len()
    } else {
        buffer.len().saturating_sub(TOKEN_LEN - 1)
    };
    let mut position = 0;
    while position < limit {
        let candidate = find(buffer, position, PREFIX).filter(|&index| index < limit);
        let Some(start) = candidate else {
            writer.write_all(&buffer[position..limit])?;
            return Ok(limit);
        };
        match token_byte(&buffer[start..]) {
            Some(byte) => {
                writer.write_all(&buffer[position..start])?;
                writer.write_all(&[byte])?;
                position = start + TOKEN_LEN;
            }
            None => {
                writer.write_all(&buffer[position..=start])?;
                position = start + 1;
            }
        }
    }
    Ok(position)
}

/// Copies `reader` to `writer` and replaces each token with its byte.
pub fn decode<R: Read, W: Write>(reader: R, mut writer: W) -> io::Result<()> {
    stream(reader, |buffer, at_end| {
        decode_step(buffer, at_end, &mut writer)
    })?;
    writer.flush()
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

    fn encoded(data: &[u8]) -> io::Result<Vec<u8>> {
        let mut output = Vec::new();
        encode(data, &mut output)?;
        Ok(output)
    }

    fn decoded(data: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        decode(data, &mut output).unwrap();
        output
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
        assert_eq!(encoded(b"\xe2\x82").unwrap(), {
            b"__MINA_BYTE_e2____MINA_BYTE_82__".to_vec()
        });
    }

    #[test]
    fn utf8_form_of_a_surrogate_is_invalid() {
        let output = encoded(b"\xed\xa0\x80").unwrap();
        assert_eq!(decoded(&output), b"\xed\xa0\x80");
        assert!(std::str::from_utf8(&output).is_ok());
    }

    #[test]
    fn encode_rejects_the_prefix_in_the_input() {
        let error = encoded(b"a __MINA_BYTE_ff__ b").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        // Also without a complete token: it can join with a real token.
        assert!(encoded(b"__MINA_BYTE_ff\xff").is_err());
    }

    #[test]
    fn encode_finds_the_prefix_across_a_chunk_border() {
        for size in 1..=PREFIX.len() + 1 {
            let reader = Trickle {
                data: b"abc__MINA_BYTE_def",
                size,
            };
            assert!(encode(reader, Vec::new()).is_err(), "read size {size}");
        }
    }

    #[test]
    fn decode_leaves_text_that_is_not_a_token() {
        for text in [
            &b"__MINA_BYTE_zz__"[..],
            b"__MINA_BYTE_FF__",
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
        assert_eq!(output.len(), input.len() - 1 + TOKEN_LEN);
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
        // xorshift, so that the test needs no dependency and a failure repeats.
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
                assert!(find(&input, 0, PREFIX).is_some());
                continue;
            };
            assert!(std::str::from_utf8(&output).is_ok());
            assert_eq!(decoded(&output), input, "input {input:?}");
            for size in [1, 2, 3, 7] {
                let mut trickled = Vec::new();
                let reader = Trickle { data: &input, size };
                encode(reader, &mut trickled).unwrap();
                assert_eq!(trickled, output, "encode read size {size}");
                let mut back = Vec::new();
                let reader = Trickle {
                    data: &output,
                    size,
                };
                decode(reader, &mut back).unwrap();
                assert_eq!(back, input, "decode read size {size}");
            }
        }
    }
}
