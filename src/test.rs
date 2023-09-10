use std::iter::once;

use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

use super::DecodeError;
use crate::test_utils::{codecs, TestDecoder, TestEncoder};

macro_rules! assert_eqb {
    ($x:expr, $y:expr) => {{
        let x: &[u8] = &$x;
        let y: &[u8] = $y;
        assert_eq!(
            x,
            y,
            "\n         {} !=\n         {}",
            repr_bytes(x),
            repr_bytes(y)
        );
    }};
}

fn print_case(encoder: &TestDecoder) {
    println!("===============");
    println!("sentinel: {}", encoder.sentinel());
    println!("max_len: {}", encoder.max_len());
    println!("---------------");
}

fn repr_bytes(bytes: impl AsRef<[u8]>) -> String {
    let repr = bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("[{repr}]")
}

fn repeat(byte: u8, count: usize) -> impl Iterator<Item = u8> {
    std::iter::repeat(byte).take(count)
}

/// A `BytesMut` only allocates in multiples of 8 bytes.
fn bytes_capacity(cap: usize) -> usize {
    if cap < 128 {
        // cap.next_multiple_of(8)  // TODO rust >= 1.73
        8 * ((cap + 7) / 8)
    } else {
        cap
    }
}

fn enc(sentinel: u8, max_len: usize, bytes: &[u8]) -> BytesMut {
    let mut output = BytesMut::new();
    let mut encoder = TestEncoder::new(sentinel, max_len);
    let result = encoder.encode(bytes, &mut output);
    assert!(result.is_ok());
    output
}

fn dec(sentinel: u8, max_len: usize, bytes: &[u8]) -> BytesMut {
    let mut encoder = TestDecoder::new(sentinel, max_len);
    encoder.decode_eof(&mut bytes.into()).unwrap().unwrap()
}

#[test]
fn encode_len() {
    assert_eq!(crate::encode_len::<0>(0), 1);
    assert_eq!(crate::encode_len::<0>(1), 2);
    assert_eq!(crate::encode_len::<0>(254), 255);

    assert_eq!(crate::encode_len::<1>(0), 0);
    assert_eq!(crate::encode_len::<1>(1), 2);
    assert_eq!(crate::encode_len::<1>(254), 255);

    assert_eq!(crate::encode_len::<255>(0), 0);
    assert_eq!(crate::encode_len::<255>(1), 1);
    assert_eq!(crate::encode_len::<255>(254), 254);
}

#[test]
fn decode_len() {
    assert_eq!(crate::decode_len::<0>(1), Some(0));
    assert_eq!(crate::decode_len::<0>(2), Some(1));
    assert_eq!(crate::decode_len::<0>(255), Some(254));
    assert_eq!(crate::decode_len::<0>(0), None);

    assert_eq!(crate::decode_len::<1>(0), Some(0));
    assert_eq!(crate::decode_len::<1>(2), Some(1));
    assert_eq!(crate::decode_len::<1>(255), Some(254));
    assert_eq!(crate::decode_len::<1>(1), None);

    assert_eq!(crate::decode_len::<255>(0), Some(0));
    assert_eq!(crate::decode_len::<255>(1), Some(1));
    assert_eq!(crate::decode_len::<255>(254), Some(254));
    assert_eq!(crate::decode_len::<255>(255), None);
}

#[test]
fn len_roundtrip() {
    for len in 0..=(super::MAX_RUN as u8) {
        for (encoder, decoder) in codecs() {
            print_case(&decoder);
            println!("len: {len}");
            let encoded_len = encoder.encode_len(len);
            println!("encoded: {encoded_len}");
            assert_ne!(encoded_len, encoder.sentinel());
            let roundtripped_len = decoder.decode_len(encoded_len);
            println!("decoded: {roundtripped_len:?}");
            match roundtripped_len {
                None => {
                    assert_eq!(len, encoder.sentinel());
                }
                Some(roundtripped_len) => {
                    assert_eq!(roundtripped_len, len);
                }
            }
        }
    }
}

#[test]
fn encode() {
    assert_eqb!(enc(0x00, 0, b""), b"\x01\x00");
    assert_eqb!(enc(0x00, 0, b"\x00"), b"\x01\x01\x00");
    assert_eqb!(enc(0x00, 0, b"\x01"), b"\x02\x01\x00");
    assert_eqb!(enc(0x00, 0, b"\xFF"), b"\x02\xFF\x00");
    assert_eqb!(enc(0x00, 0, b"\x00\x00"), b"\x01\x01\x01\x00");
    assert_eqb!(enc(0x00, 0, b"\x01\x00\x01"), b"\x02\x01\x02\x01\x00");

    assert_eqb!(enc(0x01, 0, b""), b"\x00\x01");
    assert_eqb!(enc(0x01, 0, b"\x00"), b"\x02\x00\x01");
    assert_eqb!(enc(0x01, 0, b"\x01"), b"\x00\x00\x01");
    assert_eqb!(enc(0x01, 0, b"\xFF"), b"\x02\xFF\x01");
    assert_eqb!(enc(0x01, 0, b"\x01\x01"), b"\x00\x00\x00\x01");
    assert_eqb!(enc(0x01, 0, b"\x02\x01\x02"), b"\x02\x02\x02\x02\x01");
    assert_eqb!(enc(0x01, 0, b"\xFE\x01\xFE"), b"\x02\xFE\x02\xFE\x01");
    assert_eqb!(enc(0x01, 0, b"\xFF\x01\xFF"), b"\x02\xFF\x02\xFF\x01");

    assert_eqb!(enc(0xFF, 0, b""), b"\x00\xFF");
    assert_eqb!(enc(0xFF, 0, b"\x00"), b"\x01\x00\xFF");
    assert_eqb!(enc(0xFF, 0, b"\xFE"), b"\x01\xFE\xFF");
    assert_eqb!(enc(0xFF, 0, b"\xFF"), b"\x00\x00\xFF");
    assert_eqb!(enc(0xFF, 0, b"\xFF\xFF"), b"\x00\x00\x00\xFF");
    assert_eqb!(enc(0xFF, 0, b"\x01\xFF\x01"), b"\x01\x01\x01\x01\xFF");

    let long_in = (0..=u8::MAX).collect::<BytesMut>();
    let long_out = [0x01, 0xFF]
        .into_iter()
        .chain(0x01..=0xFE)
        .chain([0x02, 0xFF, 0x00])
        .collect::<BytesMut>();
    assert_eqb!(enc(0x00, 0, &long_in), &long_out);
}

#[test]
fn decode() {
    /*
    assert_eqb!(dec(0x00, 0, b"\x01\x00"), b"");
    assert_eqb!(dec(0x00, 0, b"\x01\x01\x00"), b"\x00");
    assert_eqb!(dec(0x00, 0, b"\x02\x01\x00"), b"\x01");
    assert_eqb!(dec(0x00, 0, b"\x02\xFF\x00"), b"\xFF");
    assert_eqb!(dec(0x00, 0, b"\x01\x01\x01\x00"), b"\x00\x00");
    assert_eqb!(dec(0x00, 0, b"\x02\x01\x02\x01\x00"), b"\x01\x00\x01");

    assert_eqb!(dec(0x01, 0, b"\x00\x01"), b"");
    assert_eqb!(dec(0x01, 0, b"\x02\x00\x01"), b"\x00");
    assert_eqb!(dec(0x01, 0, b"\x00\x00\x01"), b"\x01");
    assert_eqb!(dec(0x01, 0, b"\x02\xFF\x01"), b"\xFF");
    assert_eqb!(dec(0x01, 0, b"\x00\x00\x00\x01"), b"\x01\x01");
    assert_eqb!(dec(0x01, 0, b"\x02\x02\x02\x02\x01"), b"\x02\x01\x02");
    assert_eqb!(dec(0x01, 0, b"\x02\xFE\x02\xFE\x01"), b"\xFE\x01\xFE");
    assert_eqb!(dec(0x01, 0, b"\x02\xFF\x02\xFF\x01"), b"\xFF\x01\xFF");

    assert_eqb!(dec(0xFF, 0, b"\x00\xFF"), b"");
    assert_eqb!(dec(0xFF, 0, b"\x01\x00\xFF"), b"\x00");
    assert_eqb!(dec(0xFF, 0, b"\x01\xFE\xFF"), b"\xFE");
    assert_eqb!(dec(0xFF, 0, b"\x00\x00\xFF"), b"\xFF");
    assert_eqb!(dec(0xFF, 0, b"\x00\x00\x00\xFF"), b"\xFF\xFF");
    assert_eqb!(dec(0xFF, 0, b"\x01\x01\x01\x01\xFF"), b"\x01\xFF\x01");
    */

    let long_in = (0..=u8::MAX).collect::<BytesMut>();
    let long_out = [0x01, 0xFF]
        .into_iter()
        .chain(0x01..=0xFE)
        .chain([0x02, 0xFF, 0x00])
        .collect::<BytesMut>();
    assert_eqb!(dec(0x00, 0, &long_out), &long_in);
}

#[test]
fn roundtrip() {
    let all_byte_sequence = (0..=u8::MAX).collect::<Vec<_>>();
    let messages: &[&[u8]] = &[
        &[],
        &[0x00],
        &[0x01],
        &[0x02],
        &[0xFD],
        &[0xFE],
        &[0xFF],
        &[0x00, 0x00],
        &[0x00, 0x01],
        &[0x01, 0x00],
        &[0x01, 0x01],
        &all_byte_sequence,
    ];
    for message in messages {
        for (mut encoder, mut decoder) in codecs() {
            if message.len() <= encoder.max_len() {
                print_case(&decoder);
                println!("message: {}", repr_bytes(message));
                let mut encoded_message = BytesMut::new();
                let result = encoder.encode(message, &mut encoded_message);
                assert!(result.is_ok());
                println!("encoded: {}", repr_bytes(&encoded_message));
                assert_eq!(
                    *encoded_message.last().unwrap(),
                    encoder.sentinel()
                );
                let rountripped_message =
                    decoder.decode_eof(&mut encoded_message).unwrap().unwrap();
                println!("decoded: {}", repr_bytes(&rountripped_message));
                assert_eq!(&*rountripped_message, *message);
                assert_eq!(encoded_message, b"".as_slice());
            }
        }
    }
}

#[test]
fn decode_unexpected_sentinel() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [decoder.encode_len(1), decoder.sentinel()];
        let mut bytes = bytes.as_slice().into();
        let result = decoder.decode(&mut bytes);
        match result {
            Err(DecodeError::UnexpectedSentinel) => (),
            _ => panic!("unexpected result: {result:?}"),
        }
    }
}

#[test]
fn decode_no_frame_overflow_in_chunk() {
    for mut decoder in TestDecoder::iter() {
        let max_len = decoder.max_len();
        if max_len != 0 {
            print_case(&decoder);
            // This is the encoding of a message that starts
            // with a number of sentinels and ends with two non-sentinels
            // (1 in case max-len is 1).
            // The start prevents any complications of chunks
            // around the point where the frame overflows.
            let head_len = if max_len == 1 { 0 } else { max_len - 2 };
            let body_len = if max_len == 1 { 1 } else { 2 };
            let mut bytes = repeat(decoder.encode_len(0), head_len)
                .chain(once(decoder.encode_len(body_len as u8)))
                .chain(repeat(decoder.non_sentinel(), body_len))
                .chain(once(decoder.sentinel()))
                .collect::<BytesMut>();
            assert_eq!(bytes.len(), max_len + 2);
            let decoded = decoder.decode_eof(&mut bytes).unwrap().unwrap();
            assert_eq!(decoded.len(), max_len);
            assert!(decoded.iter().enumerate().all(|(index, byte)| {
                if index + 2 < max_len {
                    *byte == decoder.sentinel()
                } else {
                    *byte == decoder.non_sentinel()
                }
            }));
        }
    }
}

#[test]
fn decode_frame_overflow_in_chunk() {
    for mut decoder in TestDecoder::iter() {
        let max_len = decoder.max_len();
        if max_len != 0 {
            print_case(&decoder);
            // This is the encoding of a message that starts
            // with a number of sentinels and ends with three non-sentinels.
            // The start prevents any complications of chunks
            // around the point where the frame overflows.
            let head_len = if max_len == 1 { 0 } else { max_len - 2 };
            let body_len = if max_len == 1 { 2 } else { 3 };
            let mut bytes = repeat(decoder.encode_len(0), head_len)
                .chain(once(decoder.encode_len(body_len as u8)))
                .chain(repeat(decoder.non_sentinel(), body_len))
                .chain(once(decoder.sentinel()))
                .collect::<BytesMut>();
            assert_eq!(bytes.len(), max_len + 3);
            let result = decoder.decode_eof(&mut bytes);
            match result {
                Err(DecodeError::FrameOverflow) => (),
                _ => panic!("unexpected result: {result:?}"),
            }
        }
    }
}

#[test]
fn decode_no_frame_overflow_between_chunk() {
    for mut decoder in TestDecoder::iter() {
        let max_len = decoder.max_len();
        if max_len != 0 {
            print_case(&decoder);
            // This is the encoding of a message that contains
            // only sentinels.
            let mut bytes = once(decoder.encode_len(0))
                .chain(repeat(decoder.encode_len(0), max_len))
                .chain(std::iter::once(decoder.sentinel()))
                .collect::<BytesMut>();
            assert!(bytes.len() == max_len + 2);
            let decoded = decoder.decode_eof(&mut bytes).unwrap().unwrap();
            assert_eq!(decoded.len(), max_len);
            assert!(decoded.iter().all(|byte| *byte == decoder.sentinel()));
        }
    }
}

#[test]
fn decode_frame_overflow_between_chunk() {
    for mut decoder in TestDecoder::iter() {
        let max_len = decoder.max_len();
        if max_len != 0 {
            print_case(&decoder);
            // This is the encoding of a message that contains
            // only sentinels so the overflow is guaranteed
            // to happen in between two chunks.
            let mut bytes = once(decoder.encode_len(0))
                .chain(repeat(decoder.encode_len(0), max_len + 1))
                .chain(std::iter::once(decoder.sentinel()))
                .collect::<BytesMut>();
            assert!(bytes.len() == max_len + 3);
            let result = decoder.decode_eof(&mut bytes);
            match result {
                Err(DecodeError::FrameOverflow) => (),
                _ => panic!("unexpected result: {result:?}"),
            }
        }
    }
}

#[test]
fn decode_missing_initial_frame() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [decoder.sentinel()];
        let mut bytes = bytes.as_slice().into();
        let result = decoder.decode_eof(&mut bytes);
        match result {
            Err(DecodeError::MissingFrame) => (),
            _ => panic!("unexpected result: {result:?}"),
        }
    }
}

#[test]
fn decode_missing_later_frame() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [
            decoder.encode_len(0),
            decoder.sentinel(),
            decoder.sentinel(),
        ];
        let mut bytes = bytes.as_slice().into();
        let first_frame = decoder.decode(&mut bytes).unwrap().unwrap();
        assert_eqb!(first_frame, b"");
        let result = decoder.decode_eof(&mut bytes);
        match result {
            Err(DecodeError::MissingFrame) => (),
            _ => panic!("unexpected result: {result:?}"),
        }
    }
}

#[test]
fn decode_recover_after_missing_frame() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [
            decoder.sentinel(),
            decoder.encode_len(0),
            decoder.encode_len(0),
            decoder.sentinel(),
        ];
        let mut bytes = bytes.as_slice().into();
        let first_result = decoder.decode(&mut bytes);
        match first_result {
            Err(DecodeError::MissingFrame) => (),
            _ => panic!("unexpected result: {first_result:?}"),
        }
        let final_frame = decoder.decode(&mut bytes).unwrap().unwrap();
        assert_eqb!(final_frame, &[decoder.sentinel()]);
    }
}

#[test]
fn decode_recover_after_unexpected_sentinel() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [
            decoder.encode_len(1),
            decoder.sentinel(),
            decoder.encode_len(0),
            decoder.encode_len(0),
            decoder.sentinel(),
        ];
        let mut bytes = bytes.as_slice().into();
        let first_result = decoder.decode(&mut bytes);
        match first_result {
            Err(DecodeError::UnexpectedSentinel) => (),
            _ => panic!("unexpected result: {first_result:?}"),
        }
        let final_frame = decoder.decode(&mut bytes).unwrap().unwrap();
        assert_eqb!(final_frame, &[decoder.sentinel()]);
    }
}

#[test]
fn decode_recover_after_frame_overflow() {
    let mut decoder = TestDecoder::new(0, 1);
    print_case(&decoder);
    let bytes = [
        decoder.encode_len(2),
        decoder.non_sentinel(),
        decoder.non_sentinel(),
        decoder.sentinel(),
        decoder.encode_len(0),
        decoder.encode_len(0),
        decoder.sentinel(),
    ];
    let mut bytes = bytes.as_slice().into();
    let first_result = decoder.decode(&mut bytes);
    match first_result {
        Err(DecodeError::FrameOverflow) => (),
        _ => panic!("unexpected result: {first_result:?}"),
    }
    let final_frame = decoder.decode(&mut bytes).unwrap().unwrap();
    assert_eqb!(final_frame, &[decoder.sentinel()]);
}

#[test]
fn decode_multiple_frames() {
    for max_len in [0, 2] {
        let mut decoder = TestDecoder::new(0, max_len);
        print_case(&decoder);
        let bytes = [
            decoder.encode_len(0),
            decoder.encode_len(0),
            decoder.sentinel(),
            decoder.encode_len(1),
            decoder.non_sentinel(),
            decoder.sentinel(),
            decoder.encode_len(1),
            decoder.non_sentinel(),
            decoder.encode_len(0),
            decoder.sentinel(),
            decoder.encode_len(0),
            decoder.encode_len(1),
            decoder.non_sentinel(),
            decoder.sentinel(),
        ];
        let mut bytes = bytes.as_slice().into();
        let frame1 = decoder.decode(&mut bytes).unwrap().unwrap();
        assert_eqb!(frame1, &[decoder.sentinel()]);
        let frame2 = decoder.decode(&mut bytes).unwrap().unwrap();
        assert_eqb!(frame2, &[decoder.non_sentinel()]);
        let frame3 = decoder.decode(&mut bytes).unwrap().unwrap();
        assert_eqb!(frame3, &[decoder.non_sentinel(), decoder.sentinel()]);
        let frame4 = decoder.decode(&mut bytes).unwrap().unwrap();
        assert_eqb!(frame4, &[decoder.sentinel(), decoder.non_sentinel()]);
        assert!(decoder.decode_eof(&mut bytes).unwrap().is_none());
    }
}

#[test]
fn decode_need_more_data_initially() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let mut bytes = BytesMut::new();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert!(bytes.is_empty());
        let max_len = decoder.max_len();
        if max_len == 0 {
            assert_ne!(bytes.capacity(), 0);
        } else {
            let capacity = crate::max_encoded_len(max_len);
            assert_eq!(bytes.capacity(), bytes_capacity(capacity));
        }
    }
}

#[test]
fn decode_need_more_data_while_reading() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [decoder.encode_len(0)];
        let mut bytes = bytes.as_slice().into();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert!(bytes.is_empty());
        let max_len = decoder.max_len();
        if max_len == 0 {
            assert_ne!(bytes.capacity(), 0);
        } else {
            let capacity = crate::max_encoded_len(max_len);
            // It seems `BytesMut` keeps the enitre capacity
            // if we remove the single byte, but only
            // for bigger allcoations.
            let delta = if max_len < 128 { 1 } else { 0 };
            assert_eq!(bytes.capacity(), bytes_capacity(capacity) - delta);
        }
        let capacity = bytes.capacity();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert_eq!(bytes.capacity(), capacity);
    }
}

#[test]
fn decode_need_more_data_at_chunk_start() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [decoder.encode_len(0)];
        let mut bytes = bytes.as_slice().into();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert!(bytes.is_empty());
        let max_len = decoder.max_len();
        if max_len == 0 {
            assert_ne!(bytes.capacity(), 0);
        } else {
            let capacity = crate::max_encoded_len(max_len);
            // It seems `BytesMut` keeps the enitre capacity
            // if we remove the single byte, but only
            // for bigger allcoations.
            let delta = if max_len < 128 { 1 } else { 0 };
            assert_eq!(bytes.capacity(), bytes_capacity(capacity) - delta);
        }
        let capacity = bytes.capacity();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert_eq!(bytes.capacity(), capacity);
    }
}

#[test]
fn decode_need_more_data_within_chunk() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [decoder.encode_len(4), decoder.non_sentinel()];
        let mut bytes = bytes.as_slice().into();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert!(bytes.is_empty());
        let max_len = decoder.max_len();
        if max_len == 0 {
            assert_ne!(bytes.capacity(), 0);
        } else {
            let capacity = crate::max_encoded_len(max_len);
            // It seems `BytesMut` keeps the enitre capacity
            // if we remove the single byte, but only for bigger allcoations.
            let delta = if max_len < 128 { 2 } else { 0 };
            assert_eq!(bytes.capacity(), bytes_capacity(capacity) - delta);
        }
        let capacity = bytes.capacity();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert_eq!(bytes.capacity(), capacity);
    }
}

#[test]
fn decode_need_more_data_at_chunk_end() {
    for mut decoder in TestDecoder::iter() {
        print_case(&decoder);
        let bytes = [decoder.encode_len(1), decoder.non_sentinel()];
        let mut bytes = bytes.as_slice().into();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert!(bytes.is_empty());
        let max_len = decoder.max_len();
        if max_len == 0 {
            assert_ne!(bytes.capacity(), 0);
        } else {
            let capacity = crate::max_encoded_len(max_len);
            // It seems `BytesMut` keeps the enitre capacity
            // if we remove a few bytes, but only for bigger allcoations.
            let delta = if max_len < 128 { 2 } else { 0 };
            assert_eq!(bytes.capacity(), bytes_capacity(capacity) - delta);
        }
        let capacity = bytes.capacity();
        assert!(decoder.decode(&mut bytes).unwrap().is_none());
        assert_eq!(bytes.capacity(), capacity);
    }
}

#[test]
fn decode_need_more_data_when_lost() {
    for mut decoder in TestDecoder::iter() {
        // NOTE: Cannot get lost without a maximum frame length.
        let max_len = decoder.max_len();
        if max_len != 0 {
            print_case(&decoder);
            let mut bytes = repeat(decoder.encode_len(0), max_len + 2)
                .collect::<BytesMut>();
            let result = decoder.decode(&mut bytes);
            match result {
                Err(DecodeError::FrameOverflow) => (),
                _ => panic!("unexpected result: {result:?}"),
            }
            assert!(bytes.is_empty());
            // NOTE: Unable to predict the exact capacity here.
            assert_ne!(bytes.capacity(), 0);
            let capacity = bytes.capacity();
            assert!(decoder.decode(&mut bytes).unwrap().is_none());
            assert_eq!(bytes.capacity(), capacity);
        }
    }
}
