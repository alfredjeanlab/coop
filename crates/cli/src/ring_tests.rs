// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use super::*;

fn collect(ring: &RingBuffer, offset: u64) -> Option<Vec<u8>> {
    ring.read_from(offset).map(|(a, b)| {
        let mut v = a.to_vec();
        v.extend_from_slice(b);
        v
    })
}

#[test]
fn empty_read() {
    let ring = RingBuffer::new(16);
    assert_eq!(collect(&ring, 0), Some(vec![]));
    assert_eq!(ring.available_from(0), 0);
}

#[test]
fn sequential_writes() {
    let mut ring = RingBuffer::new(16);
    ring.write(b"hello");
    ring.write(b" world");

    assert_eq!(collect(&ring, 0), Some(b"hello world".to_vec()));
    assert_eq!(collect(&ring, 5), Some(b" world".to_vec()));
    assert_eq!(ring.available_from(0), 11);
    assert_eq!(ring.total_written(), 11);
}

#[test]
fn wrap_around() {
    let mut ring = RingBuffer::new(8);
    ring.write(b"abcdef"); // 6 bytes, write_pos=6
    ring.write(b"ghij"); // 4 bytes wraps: write_pos=2

    // total_written=10, capacity=8, oldest=2
    // so offset 0 and 1 are gone
    assert_eq!(collect(&ring, 0), None);
    assert_eq!(collect(&ring, 1), None);
    assert_eq!(collect(&ring, 2), Some(b"cdefghij".to_vec()));
    assert_eq!(collect(&ring, 6), Some(b"ghij".to_vec()));
    assert_eq!(ring.available_from(2), 8);
}

#[test]
fn offset_too_new() {
    let mut ring = RingBuffer::new(16);
    ring.write(b"abc");
    assert_eq!(collect(&ring, 4), None);
    assert_eq!(ring.available_from(4), 0);
}

#[test]
fn exact_capacity_write() {
    let mut ring = RingBuffer::new(4);
    ring.write(b"abcd");
    assert_eq!(collect(&ring, 0), Some(b"abcd".to_vec()));
    assert_eq!(ring.total_written(), 4);
}

#[test]
fn overwrite_full_buffer() {
    let mut ring = RingBuffer::new(4);
    ring.write(b"abcd");
    ring.write(b"efgh");
    // oldest offset is 4
    assert_eq!(collect(&ring, 0), None);
    assert_eq!(collect(&ring, 4), Some(b"efgh".to_vec()));
}

#[test]
fn read_at_total_written_returns_empty() {
    let mut ring = RingBuffer::new(16);
    ring.write(b"hello");
    assert_eq!(collect(&ring, 5), Some(vec![]));
}
