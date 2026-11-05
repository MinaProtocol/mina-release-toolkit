//! Carry bytes that are not valid UTF-8 through a text JSON tool.
//!
//! Mina allows arbitrary bytes in some text fields, so its JSON is not always
//! valid UTF-8. jq replaces each invalid byte with U+FFFD, but these bytes must
//! stay exact.
//!
//! [`Encoder`] turns each invalid byte into an ASCII token, which a text tool
//! passes through. [`Decoder`] turns the tokens back into the bytes. All other
//! bytes are copied as they are. [`jq::JqInvocation`] runs jq between the two.

mod error;
pub mod jq;
mod token;
mod transcode;

pub use error::{Error, Result, UsageError};
pub use transcode::{Decoder, Encoder, Transcode};
