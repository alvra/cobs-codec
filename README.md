# Cobs-Codec

[![Documentation](https://docs.rs/cobs-codec/badge.svg)](https://docs.rs/cobs-codec/)
[![Latest version](https://img.shields.io/crates/v/cobs-codec.svg)](https://crates.io/crates/cobs-codec)
[![Build Status](https://github.com/alvra/cobs-codec/workflows/Code%20Check/badge.svg)](https://github.com/alvra/cobs-codec/actions?query=workflow%3ACode+Check)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance/)

This crate provides a COBS (Consistent Overhead Byte Stuffing) codec
for Tokio.

The COBS encoding is a very efficient framing method for network packets.
Basically; it allows you to send messages consisting of any bytes
while still being able to detect where messages start and end.

Encoded messages end with a specific (customizable) byte called a sentinel.
Any occurrence of this byte within the message is avoided by a substition
scheme that adds very litte overhead: `O(1 + N/254)` worst case.

### Choosing a Sentinel Value

This crate allows users to choose their own sentinel value.
There are two guiding principles when choosing a value.

**Size**: The encoding has the least overhead when the message
  contains one sentinel at least every 254 bytes.
 Note that this consideration is irrelevant for messages
 up to 254 bytes long.

**Speed**: Encoding/decoding is fastest for messages with as few
  sentinel values as possible, ideally none.


## Features

  * No unsafe code (`#[forbid(unsafe_code)]`)
  * Tested


## Example

``` rust
use std::io::Cursor;
use tokio_util::codec::{FramedWrite, FramedRead};
use futures::{SinkExt, StreamExt};

use cobs_codec::{Encoder, Decoder};

// Choose a message separator that does not appear too frequently in your messages.
const SENTINEL: u8 = 0x00;

// It's a good idea to limit message size to prevent running out of memory.
const MAX: usize = 32;

let encoder = Encoder::<SENTINEL, MAX>::new();
let decoder = Decoder::<SENTINEL, MAX>::new();

// Imagine this buffer being sent from the server to the client.
let mut buffer = Vec::with_capacity(128);

let mut server_cursor = Cursor::new(&mut buffer);
let mut server = FramedWrite::new(&mut server_cursor, encoder);

// Send a few messages.
assert!(server.send("hello").await.is_ok());
assert!(server.send("world").await.is_ok());

let mut client_cursor = Cursor::new(&mut buffer);
let mut client = FramedRead::new(&mut client_cursor, decoder);

// Receive the messages.
assert_eq!(convert(&client.next().await), Some(Ok(b"hello".as_slice())));
assert_eq!(convert(&client.next().await), Some(Ok(b"world".as_slice())));
assert_eq!(convert(&client.next().await), None);
```


## Documentation

[Documentation](https://lib.rs/crates/cobs-codec)


## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.


## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
