use std::{
    io::{BufRead, Read},
    slice, fmt,
};

use crate::ReadBytesExt;

pub fn decompress_rnc1<R: BufRead + ReadBytesExt>(
    r: &mut R,
) -> Result<Vec<u8>, DecompressError> {
    let mut decoder = Decoder::new(r);
    decoder.decode()?;

    Ok(decoder.output)
}

#[derive(Debug)]
pub enum DecompressError {
    Io(std::io::Error),
    SignatureError,
}

impl fmt::Display for DecompressError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            DecompressError::Io(ref err) => write!(f, "{}", err),
            DecompressError::SignatureError => write!(f, "Invalid signature"),
        }
    }
}

impl From<std::io::Error> for DecompressError {
    fn from(err: std::io::Error) -> DecompressError {
        DecompressError::Io(err)
    }
}

#[derive(Debug)]
struct Header {
    signature: [u8; 4],
    unpacked_len: u32,
    packed_len: u32,
    crc_unpacked: u16,
    crc_packed: u16,
    overlaps_size: u8,
    blocks: u8,
}

impl Header {
    fn read<R: Read + ReadBytesExt>(r: &mut R) -> Result<Self, DecompressError> {
        let mut signature = [0; 4];
        r.read_exact(&mut signature)?;

        Ok(Header {
            signature,
            unpacked_len: r.read_be_u32()?,
            packed_len: r.read_be_u32()?,
            crc_unpacked: r.read_be_u16()?,
            crc_packed: r.read_be_u16()?,
            overlaps_size: r.read_u8()?,
            blocks: r.read_u8()?,
        })
    }

    fn signature_is_valid(&self) -> bool {
        self.signature == [b'R', b'N', b'C', 0x01]
    }
}

#[derive(Copy, Clone, Default)]
struct Node {
    l3: u32,
    bit_depth: u16,
}

type Table = [Node; 16];

fn table_new() -> Table {
    [Node::default(); 16]
}

struct Decoder<R: Read + ReadBytesExt> {
    r: R,
    bit_queue: BitQueue,
    output: Vec<u8>,
}

fn inverse_bits(v: u32, count: u16) -> u32 {
    let mut v = v;
    let mut r = 0;
    for _ in 0..count {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

impl<R: BufRead + ReadBytesExt> Decoder<R> {
    fn new(r: R) -> Decoder<R> {
        Decoder {
            r,
            bit_queue: BitQueue::new(),
            output: Vec::new(),
        }
    }

    fn read_bits(&mut self, n: u8) -> std::io::Result<u16> {
        self.bit_queue.read_bits(&mut self.r, n)
    }

    fn read_table(&mut self, table: &mut Table) -> std::io::Result<()> {
        let leaf_nodes = self.read_bits(5)?.min(16) as usize;

        if leaf_nodes == 0 {
            return Ok(());
        }

        for node in table.iter_mut().take(leaf_nodes) {
            node.bit_depth = self.read_bits(4)?;
        }

        let mut val = 0;
        let mut div = 0x8000_0000u32;
        for bits_count in 1..17 {
            for node in table.iter_mut().take(leaf_nodes) {
                if node.bit_depth == bits_count {
                    node.l3 = inverse_bits(val / div, bits_count);
                    val = val.wrapping_add(div);
                }
            }
            div >>= 1;
        }

        Ok(())
    }

    fn decode(&mut self) -> Result<(), DecompressError> {
        let header = Header::read(&mut self.r)?;

        if !header.signature_is_valid() {
            return Err(DecompressError::SignatureError);
        }

        self.output = Vec::with_capacity(header.unpacked_len as usize);

        _ = self.read_bits(2)?;

        let mut raw_table = table_new();
        let mut len_table = table_new();
        let mut pos_table = table_new();

        for _ in 0..header.blocks {
            self.read_table(&mut raw_table)?;
            self.read_table(&mut len_table)?;
            self.read_table(&mut pos_table)?;

            let subchunks = self.bit_queue.read_bits(&mut self.r, 16)?;

            for subchunk in 0..subchunks {
                let mut input_length = self.input_value(&raw_table)? as usize;

                if input_length > 0 {
                    let mut b = 0u8;
                    while input_length > 0 {
                        self.r.read_exact(slice::from_mut(&mut b))?;
                        self.output.push(b);

                        input_length -= 1;
                    }
                }

                if subchunk < subchunks - 1{
                    let match_offset = (self.input_value(&len_table)? + 1) as usize;
                    let match_count = (self.input_value(&pos_table)? + 2) as usize;

                    let len = self.output.len();
                    for j in 0..match_count {
                        let b = self.output[len - match_offset + j];
                        self.output.push(b);
                    }
                }
            }
        }

        Ok(())
    }

    fn input_value(&mut self, table: &Table) -> std::io::Result<u16> {
        for i in 0u16.. {
            let node = &table[i as usize];
            if node.bit_depth == 0 {
                continue;
            }

            let peek = self.bit_queue.peek(&mut self.r)?;
            let mask = (1 << node.bit_depth) - 1;

            if node.l3 == (peek & mask) as u32 {
                self.bit_queue
                    .read_bits(&mut self.r, node.bit_depth as u8)?;

                if i < 2 {
                    return Ok(i);
                }

                let v = self.bit_queue.read_bits(&mut self.r, (i - 1) as u8)?;
                let v = v | (1 << (i - 1));
                return Ok(v);
            }
        }
        unreachable!();
    }
}

struct BitQueue {
    bit_queue: u32,
    bits_in_queue: u16,
}

impl BitQueue {
    fn new() -> Self {
        BitQueue {
            bit_queue: 0,
            bits_in_queue: 0,
        }
    }

    #[inline]
    fn refill<R: Read + ReadBytesExt>(&mut self, r: &mut R) -> std::io::Result<()> {
        // We read two u8's instead of one u16 because read_le_u8 will
        // fail if onle one byte is left in the input stream.
        let b0 = r.read_u8().unwrap_or(0) as u32;
        let b1 = r.read_u8().unwrap_or(0) as u32;

        let new_bits = (b1 << 8) | b0;

        self.bit_queue |= new_bits << self.bits_in_queue;
        self.bits_in_queue += 16;

        Ok(())
    }

    #[inline]
    fn peek<R: BufRead + ReadBytesExt>(&mut self, r: &mut R) -> std::io::Result<u16> {
        let peek = r.fill_buf()?;
        let p0 = *peek.first().unwrap_or(&0) as u32;
        let p1 = *peek.get(1).unwrap_or(&0) as u32;
        let p = (p1 << 8) | p0;
        let t = ((p << self.bits_in_queue) | self.bit_queue) as u16;

        Ok(t)
    }

    fn read_bits<R: Read + ReadBytesExt>(&mut self, r: &mut R, n: u8) -> std::io::Result<u16> {
        assert!(n <= 16);
        let n = n as u16;

        if n > self.bits_in_queue {
            self.refill(r)?;
        }

        let mask = ((1u32 << n) - 1) as u16;
        let v = (self.bit_queue as u16) & mask;

        self.bit_queue >>= n;
        self.bits_in_queue -= n;

        Ok(v)
    }
}
