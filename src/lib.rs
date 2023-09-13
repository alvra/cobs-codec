//! This crate provides a COBS (Consistent Overhead Byte Stuffing) codec
//! for Tokio.
//!
//! The COBS encoding is a very efficient framing method for network packets.
//! Basically; it allows you to send messages consisting of any bytes
//! while still being able to detect where messages start and end.
//!
//! This is achieved by ending encoded messages with a specific
//! (customizable) byte called a sentinel.
//! Any occurrence of this byte within the message is avoided by a substition
//! scheme that adds very litte overhead: `O(1 + N/254)` worst case.
//!
//! See [the Wikipedia acticle on COBS][wiki] for details.
//!
//! [wiki]: https://www.wikipedia.org/wiki/Consistent_Overhead_Byte_Stuffing
//!
//! ### Choosing a Sentinel Value
//!
//! This crate allows users to choose their own sentinel value.
//! There are two guiding principles when choosing a value.
//!
//! **Size**: The encoding has the least overhead when the message
//!   contains one sentinel at least every 254 bytes.
//!  Note that this consideration is irrelevant for messages
//!  up to 254 bytes long.
//!
//! **Speed**: Encoding/decoding is fastest for messages with as few
//!   sentinel values as possible, ideally none.
//!
//!
//! # Examples
//!
//! ```
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! use std::io::Cursor;
//! use tokio_util::codec::{FramedWrite, FramedRead};
//! use futures::{SinkExt, StreamExt};
//!
//! use cobs_codec::{Encoder, Decoder};
//!
//! // Choose a message separator that does not appear too frequently in your messages.
//! const SENTINEL: u8 = 0x00;
//!
//! // It's a good idea to limit message size to prevent running out of memory.
//! const MAX: usize = 32;
//!
//! let encoder = Encoder::<SENTINEL, MAX>::new();
//! let decoder = Decoder::<SENTINEL, MAX>::new();
//!
//! // Imagine this buffer being sent from the server to the client.
//! let mut buffer = Vec::with_capacity(128);
//!
//! let mut server_cursor = Cursor::new(&mut buffer);
//! let mut server = FramedWrite::new(&mut server_cursor, encoder);
//!
//! // Send a few messages.
//! assert!(server.send("hello").await.is_ok());
//! assert!(server.send("world").await.is_ok());
//!
//! let mut client_cursor = Cursor::new(&mut buffer);
//! let mut client = FramedRead::new(&mut client_cursor, decoder);
//!
//! // Receive the messages.
//! assert_eq!(convert(&client.next().await), Some(Ok(b"hello".as_slice())));
//! assert_eq!(convert(&client.next().await), Some(Ok(b"world".as_slice())));
//! assert_eq!(convert(&client.next().await), None);
//! # fn convert<E>(
//! #     bytes: &Option<Result<bytes::BytesMut, E>>,
//! # ) -> Option<Result<&[u8], ()>> {
//! #     bytes
//! #         .as_ref()
//! #         .map(|res| res.as_ref().map(|bytes| bytes.as_ref()).map_err(|_| ()))
//! # }
//! # }
//! ```

#![forbid(unsafe_code)]

use bytes::{Buf, BufMut, BytesMut};

#[cfg(test)]
mod test_utils;

#[cfg(test)]
mod test;

const DEFAULT_SENTINEL: u8 = 0x00;
const DEFAULT_MAX_LEN: usize = 0;

/// The decode output buffer size if there is no frame length limit.
const DEFAULT_DECODE_BUFFER_CAPACITY: usize = 8 * 1024;

const MAX_RUN: usize = 254;

const fn max_encoded_len(input_len: usize) -> usize {
    let overhead = if input_len == 0 {
        // In the special case of an empty message, we wind up generating one
        // byte of overhead.
        1
    } else {
        (input_len + 253) / 254
    };
    // +1 for terminator byte.
    input_len + overhead + 1
}

const fn decode_buffer_cap(max_len: usize) -> usize {
    if max_len == 0 {
        // use a reasonable default in case the frame size is unlimited
        DEFAULT_DECODE_BUFFER_CAPACITY
    } else {
        max_len
    }
}

/// Encoding a len (between `0` and `MAX_RUN` inclusive) into a byte such that
/// we avoid `SENTINEL`.
#[inline(always)]
fn encode_len<const SENTINEL: u8>(len: usize) -> u8 {
    debug_assert!(len <= MAX_RUN);
    // We're doing the addition on `usize` to ensure we don't generate
    // additional zero extend instructions.
    #[allow(clippy::collapsible_else_if)]
    if SENTINEL == 0 {
        len.wrapping_add(1) as u8
    } else if SENTINEL == 255 {
        assert!(SENTINEL as usize > MAX_RUN);
        len as u8
    } else {
        if len >= SENTINEL as usize {
            len.wrapping_add(1) as u8
        } else {
            len as u8
        }
    }
}

/// Decodes a length-or-terminator byte. If the byte is `SENTINEL`, returns `None`.
/// Otherwise returns the length of the run encoded by the byte.
#[inline(always)]
fn decode_len<const SENTINEL: u8>(code: u8) -> Option<usize> {
    let len = if SENTINEL == 0 {
        usize::from(code).checked_sub(1)
    } else if SENTINEL == 255 {
        if code == SENTINEL {
            None
        } else {
            Some(usize::from(code))
        }
    } else {
        use std::cmp::Ordering;
        match code.cmp(&SENTINEL) {
            Ordering::Equal => None,
            Ordering::Less => Some(usize::from(code)),
            Ordering::Greater => Some(usize::from(code).wrapping_sub(1)),
        }
    };
    if let Some(len) = len {
        debug_assert!(len <= MAX_RUN);
    };
    len
}

#[inline(always)]
fn encode<const SENTINEL: u8, const MAX_LEN: usize>(
    input: &[u8],
    output: &mut BytesMut,
) {
    output.reserve(max_encoded_len(input.len()));
    if MAX_LEN != 0 {
        debug_assert!(input.len() <= MAX_LEN);
    }
    if MAX_LEN != 0 && MAX_LEN <= MAX_RUN {
        // The input is small enough to never need multiple chunks.
        for run in input.split(|&b| b == SENTINEL) {
            output.put_u8(encode_len::<SENTINEL>(run.len()));
            output.put_slice(run);
        }
    } else {
        let mut prev_run_was_maximal = false;

        // The encoding process can be described in terms of "runs" of non-zero
        // bytes in the input data. We process each run individually.
        //
        // Currently, the scanning-for-zeros loop here is the hottest part of the
        // encode profile.
        for mut run in input.split(|&b| b == SENTINEL) {
            // If the last run we encoded was maximal length, we need to encode an
            // explicit zero between it and our current `run`.
            if prev_run_was_maximal {
                output.put_u8(encode_len::<SENTINEL>(0));
            }

            // We can only encode a run of up to `MAX_RUN` bytes in COBS. This may
            // require us to split `run` into multiple output chunks -- in the
            // extreme case, if the input contains no zeroes, we'll process all of
            // it here.
            loop {
                let chunk_len = usize::min(run.len(), MAX_RUN);
                let (chunk, new_run) = run.split_at(chunk_len);
                output.put_u8(encode_len::<SENTINEL>(chunk_len));
                output.put_slice(chunk);

                run = new_run;
                prev_run_was_maximal = chunk_len == MAX_RUN;

                // We test this condition here, rather than as a `while` loop,
                // because we want to process empty runs once.
                if run.is_empty() {
                    break;
                }
            }
        }
    }
    output.put_u8(SENTINEL);
}

/// Frame encoder.
///
/// This type implements [`Encoder<impl AsRef<[i8]>>`](tokio_util::codec::Encoder);
/// it encodes any message type that be converted to a byte slice
/// using [`AsRef<[u8]>`](AsRef).
///
/// This type can be customized via generic parameters:\
/// *`SENTINEL`*: Choose a byte to be used as a frame separator.
///   The corresponding [`Decoder`] must use the same value.
///   Refer to the crate documentation for more details on choosing a sentinel.\
/// *`MAX_LEN`*: Choose the maximum size of a message,
///   or set to `0` for unlimited message sizes.
///   This parameter is used as an optimization.
///   If any message exceeds this limit, encoding will panic.
#[derive(Default, Debug)]
pub struct Encoder<
    const SENTINEL: u8 = DEFAULT_SENTINEL,
    const MAX_LEN: usize = DEFAULT_MAX_LEN,
>;

impl<const SENTINEL: u8, const MAX_LEN: usize> Encoder<SENTINEL, MAX_LEN> {
    /// Create a new encoder.
    pub fn new() -> Self {
        Self
    }
}

impl<const SENTINEL: u8, const MAX_LEN: usize, T: AsRef<[u8]>>
    tokio_util::codec::Encoder<T> for Encoder<SENTINEL, MAX_LEN>
{
    type Error = std::io::Error;

    #[inline(always)]
    fn encode(
        &mut self,
        item: T,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let bytes = item.as_ref();
        assert!(MAX_LEN == 0 || bytes.len() <= MAX_LEN);
        encode::<SENTINEL, MAX_LEN>(bytes, dst);
        assert_eq!(dst.last(), Some(&SENTINEL));
        Ok(())
    }
}

#[derive(Debug)]
enum DecoderReadResult {
    NeedMoreData,
    Frame(BytesMut),
    UnexpectedSentinel,
    FrameOverflow,
}

#[derive(Debug)]
struct DecoderReadingState {
    next_chunk_offset: usize,
    output: BytesMut,
    chunk_overflow: bool,
}

impl DecoderReadingState {
    #[inline(always)]
    fn new<const MAX_LEN: usize>(offset: usize) -> Self {
        let mut this = Self {
            next_chunk_offset: 0,
            output: BytesMut::with_capacity(decode_buffer_cap(MAX_LEN)),
            chunk_overflow: false,
        };
        this.update(offset);
        this
    }

    #[inline(always)]
    fn update(&mut self, offset: usize) {
        self.next_chunk_offset = offset;
        self.chunk_overflow = offset == MAX_RUN;
    }

    #[inline(always)]
    fn read<const SENTINEL: u8, const MAX_LEN: usize>(
        &mut self,
        src: &mut BytesMut,
    ) -> DecoderReadResult {
        loop {
            if src.is_empty() {
                return DecoderReadResult::NeedMoreData;
            }
            // Process the remainder of a chunk.
            if self.next_chunk_offset > 0 {
                let len = usize::min(self.next_chunk_offset, src.len());
                if MAX_LEN != 0 && self.output.len() + len > MAX_LEN {
                    return DecoderReadResult::FrameOverflow;
                }
                self.next_chunk_offset -= len;
                let chunk = src.split_to(len);
                if chunk.contains(&SENTINEL) {
                    return DecoderReadResult::UnexpectedSentinel;
                }
                self.output.put(chunk);
                if src.is_empty() {
                    return DecoderReadResult::NeedMoreData;
                }
            }
            // Process the start of a new chunk.
            debug_assert!(self.next_chunk_offset == 0);
            debug_assert!(!src.is_empty());
            if let Some(offset) = decode_len::<SENTINEL>(src.get_u8()) {
                if !self.chunk_overflow {
                    if MAX_LEN != 0 && self.output.len() == MAX_LEN {
                        return DecoderReadResult::FrameOverflow;
                    }
                    self.output.put_u8(SENTINEL);
                }
                self.update(offset);
            } else {
                // The frame is complete.
                let capacity = decode_buffer_cap(MAX_LEN);
                let new_output = BytesMut::with_capacity(capacity);
                let frame = std::mem::replace(&mut self.output, new_output);
                return DecoderReadResult::Frame(frame);
            }
        }
    }
}

#[derive(Debug)]
enum DecoderState {
    Initial,
    Reading(DecoderReadingState),
    Lost,
}

/// Frame decoder.
///
/// This type implements [`Decoder`](tokio_util::codec::Decoder);
/// it decodes into [`BytesMut`].
///
/// This type can be customized via generic parameters:\
/// *`SENTINEL`*: Choose a byte to be used as a frame separator.
///   The corresponding [`Encoder`] must use the same value.
///   Refer to the crate documentation for more details on choosing a sentinel.\
/// *`MAX_LEN`*: Choose the maximum size of a message,
///   or set to `0` for unlimited message sizes.
///   This parameter is used as a safety measure to prevent
///   running out of memory. If any message exceeds this limit,
///   decoding will return [`DecodeError::FrameOverflow`].
#[derive(Debug)]
pub struct Decoder<
    const SENTINEL: u8 = DEFAULT_SENTINEL,
    const MAX_LEN: usize = DEFAULT_MAX_LEN,
> {
    state: DecoderState,
}

impl<const SENTINEL: u8, const MAX_LEN: usize> Decoder<SENTINEL, MAX_LEN> {
    /// Create a new decoder.
    pub fn new() -> Self {
        Self {
            state: DecoderState::Initial,
        }
    }
}

impl<const SENTINEL: u8, const MAX_LEN: usize> Default
    for Decoder<SENTINEL, MAX_LEN>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Error while decoding.
#[derive(thiserror::Error, Debug)]
pub enum DecodeError {
    /// An error occured while reading from the underlying IO object.
    ///
    /// This variant is not used by this crate itself
    /// since decoding does not interact with IO,
    /// but is required to implement [`Decoder`](tokio_util::codec::Decoder)
    /// because [`FramedRead`](tokio_util::codec::FramedRead)
    /// wraps both the decoder and the IO object and needs
    /// to present a single error type.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A frame was found to start with a sentinel byte.
    ///
    /// This variant indicates corrupted data,
    /// either by the sender or during transmission.
    #[error("missing frame")]
    MissingFrame,
    /// The sentinel byte was found in an invalid position.
    ///
    /// This variant indicates corrupted data,
    /// either by the sender or during transmission.
    #[error("unexpected sentinel")]
    UnexpectedSentinel,
    /// The frame was longer than the limit.
    ///
    /// This variant is never returned by unlimited decoders.
    ///
    /// Either the data was corrupted during transmission,
    /// or the sender encoded a frame that exceeds the limit.
    #[error("frame overflow")]
    FrameOverflow,
}

impl<const SENTINEL: u8, const MAX_LEN: usize> tokio_util::codec::Decoder
    for Decoder<SENTINEL, MAX_LEN>
{
    type Item = BytesMut;
    type Error = DecodeError;

    fn decode(
        &mut self,
        src: &mut BytesMut,
    ) -> Result<Option<BytesMut>, Self::Error> {
        loop {
            if matches!(self.state, DecoderState::Initial) {
                src.reserve(max_encoded_len(decode_buffer_cap(MAX_LEN)));
                if src.is_empty() {
                    // Need more data to start a new frame.
                    return Ok(None);
                } else if let Some(offset) =
                    decode_len::<SENTINEL>(src.get_u8())
                {
                    // The first byte of a frame is the offset to the next sentinel
                    // value in the frame or the sentinel that marks its end.
                    let read_state =
                        DecoderReadingState::new::<MAX_LEN>(offset);
                    self.state = DecoderState::Reading(read_state);
                } else {
                    // A frame may not start with a sentinel value.
                    //
                    // Either this is the first byte received
                    // or it follows a previous sentinal that ended the last frame.
                    //
                    // Note that this case could be used to send a signal
                    // distinct from any other message.
                    return Err(DecodeError::MissingFrame);
                }
            }
            match &mut self.state {
                DecoderState::Initial => unreachable!(),
                DecoderState::Reading(state) => {
                    match state.read::<SENTINEL, MAX_LEN>(src) {
                        DecoderReadResult::NeedMoreData => return Ok(None),
                        DecoderReadResult::Frame(frame) => {
                            self.state = DecoderState::Initial;
                            return Ok(Some(frame));
                        }
                        DecoderReadResult::UnexpectedSentinel => {
                            self.state = DecoderState::Initial;
                            return Err(DecodeError::UnexpectedSentinel);
                        }
                        DecoderReadResult::FrameOverflow => {
                            self.state = DecoderState::Lost;
                            return Err(DecodeError::FrameOverflow);
                        }
                    }
                }
                DecoderState::Lost => {
                    if let Some(index) =
                        src.iter().position(|byte| *byte == SENTINEL)
                    {
                        let _ = src.split_to(index + 1);
                        let total_capacity =
                            max_encoded_len(decode_buffer_cap(MAX_LEN));
                        src.reserve(total_capacity.saturating_sub(src.len()));
                        self.state = DecoderState::Initial;
                    } else {
                        src.clear();
                        return Ok(None);
                    }
                }
            }
        }
    }
}

/// Frame codec.
///
/// This type contains both an [`Encoder`] and a [`Decoder`]
/// and implements [`Encoder`](tokio_util::codec::Encoder)
/// as well as [`Decoder`](tokio_util::codec::Decoder).
///
/// Refer to the underlying encoder and decoder types
/// for details on the generic parameters.
#[derive(Debug)]
pub struct Codec<
    const SENTINEL_ENCODE: u8 = DEFAULT_SENTINEL,
    const SENTINEL_DECODE: u8 = DEFAULT_SENTINEL,
    const MAX_LEN_ENCODE: usize = DEFAULT_MAX_LEN,
    const MAX_LEN_DECODE: usize = DEFAULT_MAX_LEN,
> {
    encoder: Encoder<SENTINEL_ENCODE, MAX_LEN_ENCODE>,
    decoder: Decoder<SENTINEL_DECODE, MAX_LEN_DECODE>,
}

impl<
        const SENTINEL_ENCODE: u8,
        const SENTINEL_DECODE: u8,
        const MAX_LEN_ENCODE: usize,
        const MAX_LEN_DECODE: usize,
    > Codec<SENTINEL_ENCODE, SENTINEL_DECODE, MAX_LEN_ENCODE, MAX_LEN_DECODE>
{
    /// Create a new codec.
    pub fn new() -> Self {
        Self {
            encoder: Encoder::new(),
            decoder: Decoder::new(),
        }
    }
}

impl<
        const SENTINEL_ENCODE: u8,
        const SENTINEL_DECODE: u8,
        const MAX_LEN_ENCODE: usize,
        const MAX_LEN_DECODE: usize,
    > Default
    for Codec<SENTINEL_ENCODE, SENTINEL_DECODE, MAX_LEN_ENCODE, MAX_LEN_DECODE>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
        const SENTINEL_ENCODE: u8,
        const SENTINEL_DECODE: u8,
        const MAX_LEN_ENCODE: usize,
        const MAX_LEN_DECODE: usize,
        T: AsRef<[u8]>,
    > tokio_util::codec::Encoder<T>
    for Codec<SENTINEL_ENCODE, SENTINEL_DECODE, MAX_LEN_ENCODE, MAX_LEN_DECODE>
{
    type Error = std::io::Error;

    fn encode(
        &mut self,
        item: T,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        self.encoder.encode(item, dst)
    }
}

impl<
        const SENTINEL_ENCODE: u8,
        const SENTINEL_DECODE: u8,
        const MAX_LEN_ENCODE: usize,
        const MAX_LEN_DECODE: usize,
    > tokio_util::codec::Decoder
    for Codec<SENTINEL_ENCODE, SENTINEL_DECODE, MAX_LEN_ENCODE, MAX_LEN_DECODE>
{
    type Item = BytesMut;
    type Error = DecodeError;

    fn decode(
        &mut self,
        src: &mut BytesMut,
    ) -> Result<Option<BytesMut>, Self::Error> {
        self.decoder.decode(src)
    }
}
