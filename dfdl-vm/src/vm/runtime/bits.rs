use crate::schema::{BitOrder, ByteOrder};
use alloc::vec;
use alloc::vec::Vec;

pub(crate) fn bit_mask(width: usize) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

pub(crate) fn reverse_field_bits(v: u64, bit_width: usize) -> u64 {
    let mut r = 0u64;
    for i in 0..bit_width {
        if (v >> i) & 1 != 0 {
            r |= 1 << (bit_width - 1 - i);
        }
    }
    r
}

pub(crate) fn stream_raw_to_msbf_bytes(raw: u64, bit_width: usize) -> Vec<u8> {
    let nbytes = bit_width.div_ceil(8);
    let mut bytes = vec![0u8; nbytes];
    for i in 0..bit_width {
        let bit = (raw >> (bit_width - 1 - i)) & 1;
        bytes[i / 8] |= (bit as u8) << (7 - (i % 8));
    }
    bytes
}

pub(crate) fn encode_packed_bit_field_bytes(
    value: u64,
    bit_width: usize,
    byte_order: ByteOrder,
    bit_order: BitOrder,
) -> Vec<u8> {
    if bit_width == 0 {
        return Vec::new();
    }
    let mut v = value & bit_mask(bit_width);
    let byte_len = bit_width.div_ceil(8);
    let fragment = bit_width % 8;
    let mut buf = vec![0u8; byte_len];
    if fragment != 0
        && byte_order == ByteOrder::BigEndian
        && bit_order == BitOrder::MostSignificantBitFirst
    {
        v <<= 8 - fragment;
    }
    for i in (0..byte_len).rev() {
        buf[i] = (v & 0xff) as u8;
        v >>= 8;
    }
    if fragment != 0 {
        if byte_order == ByteOrder::LittleEndian && bit_order == BitOrder::MostSignificantBitFirst {
            if let Some(first) = buf.first_mut() {
                *first <<= 8 - fragment;
            }
        } else if byte_order == ByteOrder::BigEndian
            && bit_order == BitOrder::LeastSignificantBitFirst
        {
            if let Some(last) = buf.last_mut() {
                *last >>= 8 - fragment;
            }
        }
    }
    if bit_width > 8 && byte_order == ByteOrder::LittleEndian {
        buf.reverse();
    }
    buf
}

pub(crate) fn decode_packed_bit_field_u64(
    bytes: &[u8],
    bit_width: usize,
    byte_order: ByteOrder,
    bit_order: BitOrder,
) -> u64 {
    if bit_width == 0 {
        return 0;
    }
    let mut buf = bytes.to_vec();
    if bit_width > 8 && byte_order == ByteOrder::LittleEndian {
        buf.reverse();
    }
    let fragment = bit_width % 8;
    if fragment != 0 {
        if byte_order == ByteOrder::LittleEndian && bit_order == BitOrder::MostSignificantBitFirst {
            if let Some(first) = buf.first_mut() {
                *first >>= 8 - fragment;
            }
        } else if byte_order == ByteOrder::BigEndian
            && bit_order == BitOrder::LeastSignificantBitFirst
        {
            if let Some(last) = buf.last_mut() {
                *last <<= 8 - fragment;
            }
        }
    }
    let mut v = 0u64;
    for b in &buf {
        v = (v << 8) | u64::from(*b);
    }
    if fragment != 0
        && byte_order == ByteOrder::BigEndian
        && bit_order == BitOrder::MostSignificantBitFirst
    {
        v >>= 8 - fragment;
    }
    v & bit_mask(bit_width)
}

pub(crate) fn decode_unsigned_binary_bytes(bytes: &[u8], le: bool) -> u64 {
    let mut value = 0u64;
    if le {
        for (i, byte) in bytes.iter().enumerate() {
            value |= (*byte as u64) << (i * 8);
        }
    } else {
        for byte in bytes {
            value = (value << 8) | (*byte as u64);
        }
    }
    value
}
