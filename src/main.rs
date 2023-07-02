#![allow(dead_code)]

mod bytes_ext;
mod rnc_decompress;

use std::{
    fmt,
    fs::File,
    io::{BufReader, Cursor, Read, Seek, Write},
};

use clap::Parser;
use csv::Writer;
use serde::Serialize;

use bytes_ext::{ReadBytesExt, WriteBytesExt};
use rnc_decompress::decompress_rnc1;

/// Extracts and decodes data files from Beneath a Steel Sky
#[derive(Parser)]
struct Cli {
    /// Path to game data files
    path: std::path::PathBuf,

    /// Dump the resource list to `resource.csv`
    #[arg(short, long, default_value_t = false)]
    dump_csv: bool,
}

#[derive(Copy, Clone, Debug)]
struct Entry {
    number: u16,
    offset: u32,
    size: u32,
    has_file_header: bool,
    uses_file_header: bool,
}

#[derive(Debug, Serialize)]
struct Header {
    flags: u16,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    sp_size: u16,
    tot_size: u16,
    n_sprites: u16,
    offset_x: i16,
    offset_y: i16,
    compressed_size: u16,
}

impl Header {
    fn is_compressed(&self) -> bool {
        self.flags & 0x80 != 0
    }
}

struct Resource {
    entry: Entry,
    header: Option<Header>,
    data: Vec<u8>,
}

impl fmt::Debug for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resource")
            .field("entry", &self.entry)
            .field("header", &self.header)
            .finish()
    }
}

impl Resource {
    fn is_compressed(&self) -> bool {
        self.header
            .as_ref()
            .map(|h| h.is_compressed())
            .unwrap_or(false)
    }
}

fn read_dinner_table<R: Read + ReadBytesExt>(file: &mut R) -> std::io::Result<Vec<Entry>> {
    let entry_count = file.read_le_u32()?;

    let mut directory = Vec::with_capacity(entry_count as usize);
    for _ in 0..entry_count {
        let number = file.read_le_u16()?;
        let offset = file.read_le_u24()?;
        let size = file.read_le_u24()?;

        let has_file_header = size >> 23 == 0;
        let uses_file_header = size >> 22 == 0;
        let size = size & 0x3f_ff_ff;

        directory.push(Entry {
            number,
            offset,
            size,
            has_file_header,
            uses_file_header,
        });
    }

    Ok(directory)
}

fn read_entry<R: Read + Seek + ReadBytesExt>(
    entry: &Entry,
    file: &mut R,
) -> std::io::Result<Vec<u8>> {
    file.seek(std::io::SeekFrom::Start(entry.offset as u64))?;

    let mut buf = Vec::<u8>::new();
    file.take(entry.size as u64).read_to_end(&mut buf)?;

    Ok(buf)
}

fn read_resource(entry: &Entry, data: Vec<u8>) -> std::io::Result<Resource> {
    if !entry.has_file_header {
        return Ok(Resource {
            entry: *entry,
            header: None,
            data,
        });
    }

    let mut r = Cursor::new(data);
    let header = Header {
        flags: r.read_le_u16()?,
        x: r.read_le_u16()?,
        y: r.read_le_u16()?,
        width: r.read_le_u16()?,
        height: r.read_le_u16()?,
        sp_size: r.read_le_u16()?,
        tot_size: r.read_le_u16()?,
        n_sprites: r.read_le_u16()?,
        offset_x: r.read_le_i16()?,
        offset_y: r.read_le_i16()?,
        compressed_size: r.read_le_u16()?,
    };

    let data = if header.is_compressed() {
        let uncompressed_data = decompress_rnc1(&mut r).ok();
        uncompressed_data.unwrap_or_else(|| {
            let mut data = Vec::new();
            r.read_to_end(&mut data).unwrap();
            data
        })
    } else {
        let mut data = Vec::new();
        r.read_to_end(&mut data)?;
        data
    };

    Ok(Resource {
        entry: *entry,
        header: Some(header),
        data,
    })
}

fn dump_entry<R: Read + Seek + ReadBytesExt>(file: &mut R, entry: &Entry) -> std::io::Result<()> {
    let buf = read_entry(entry, file)?;

    let dump_name = format!("dump/raw/{:05}.dmp", entry.number);
    let mut dump_file = File::create(dump_name)?;
    dump_file.write_all(&buf)?;

    Ok(())
}

#[inline]
fn rescale_6_bit_color_to_8_bit(c: u8) -> u8 {
    ((255 * c as u16) / 63) as u8
}

fn dump_resource_as_pal(resource: &Resource) -> std::io::Result<()> {
    let data: &Vec<u8> = &resource.data;

    const SCALE: usize = 16;

    let mut image_buffer = vec![0u8; 16 * 16 * SCALE * SCALE * 3];
    for y in 0..16 * SCALE {
        for x in 0..16 * SCALE {
            let out_ofs = 16 * SCALE * y + x;
            let in_ofs = 16 * (y / 16) + (x / 16);

            for j in 0..3 {
                image_buffer[3 * out_ofs + j] = rescale_6_bit_color_to_8_bit(data[3 * in_ofs + j])
            }
        }
    }

    let dump_name = format!("dump/palette/{:05}.ppm", resource.entry.number);
    let mut dump_file = File::create(dump_name)?;
    writeln!(dump_file, "P6 256 256 255")?;
    dump_file.write_all(&image_buffer)?;

    Ok(())
}

fn dump_screen_in_grayscale<R: Read + ReadBytesExt + Seek>(
    screen: &Entry,
    mut file: &mut R,
) -> std::io::Result<()> {
    let data = read_entry(screen, &mut file).expect("failed to read resource entry");
    let screen_res = read_resource(screen, data)?;

    let mut image_buffer = vec![0; 3 * 320 * 200];
    for y in 0..200 {
        for x in 0..320 {
            let c = screen_res.data[320 * y + x];
            for n in 0..3 {
                image_buffer[3 * (320 * y + x) + n] = c;
            }
        }
    }

    let dump_name = format!("dump/screen/{:05}-grayscale.ppm", screen_res.entry.number);
    let mut dump_file = File::create(dump_name)?;
    writeln!(dump_file, "P6 320 200 255")?;
    dump_file.write_all(&image_buffer)?;

    Ok(())
}

fn dump_screen_with_pal<R: Read + ReadBytesExt + Seek>(
    screen: &Entry,
    pal: &Entry,
    mut file: &mut R,
) -> std::io::Result<()> {
    let data = read_entry(screen, &mut file).expect("failed to read resource entry");
    let screen_res = read_resource(screen, data)?;

    let data = read_entry(pal, &mut file).expect("failed to read resource entry");
    let pal_res = read_resource(pal, data)?;

    let mut image_buffer = vec![0; 3 * 320 * 200];
    for y in 0..200 {
        for x in 0..320 {
            for n in 0..3 {
                let c = screen_res.data[320 * y + x] as usize;
                image_buffer[3 * (320 * y + x) + n] =
                    rescale_6_bit_color_to_8_bit(pal_res.data[3 * c + n]);
            }
        }
    }

    let dump_name = format!("dump/screen/{:05}.ppm", screen_res.entry.number);
    let mut dump_file = File::create(dump_name)?;
    writeln!(dump_file, "P6 320 200 255")?;
    dump_file.write_all(&image_buffer)?;

    Ok(())
}

fn get_resource_by_id<R: Read + ReadBytesExt + Seek>(
    id: u16,
    directory: &[Entry],
    file: &mut R,
) -> Option<Resource> {
    let entry = directory.iter().find(|&e| e.number == id)?;
    let data = read_entry(entry, file).ok()?;
    read_resource(entry, data).ok()
}

fn dump_audio<R: Read + ReadBytesExt + Seek>(
    entry: &Entry,
    mut file: &mut R,
) -> std::io::Result<()> {
    let data = read_entry(entry, &mut file).expect("failed to read entry");

    let resource = read_resource(entry, data).expect("failed to read resource");
    let data = resource.data;
    let data_len = data.len() as u32;

    let audio_format = 1;
    let sample_rate = 11025;
    let num_channels = 1;
    let bytes_per_sample = 1;
    let byte_rate = sample_rate * num_channels * bytes_per_sample;
    let block_align = num_channels * bytes_per_sample;
    let bits_per_sample = bytes_per_sample * 8;

    let dump_name = format!("dump/audio/{:05}.wav", entry.number);
    let mut dump_file = File::create(dump_name)?;
    dump_file.write_all(&[b'R', b'I', b'F', b'F'])?;
    dump_file.write_le_u32(data_len + 36)?;
    dump_file.write_all(&[b'W', b'A', b'V', b'E'])?;

    dump_file.write_all(&[b'f', b'm', b't', b' '])?;
    dump_file.write_le_u32(16)?;
    dump_file.write_le_u16(audio_format)?;
    dump_file.write_le_u16(num_channels as u16)?;
    dump_file.write_le_u32(sample_rate)?;
    dump_file.write_le_u32(byte_rate)?;
    dump_file.write_le_u16(block_align as u16)?;
    dump_file.write_le_u16(bits_per_sample as u16)?;

    dump_file.write_all(&[b'd', b'a', b't', b'a'])?;
    dump_file.write_le_u32(data_len)?;
    dump_file.write_all(&data)
}

#[derive(Debug, Serialize)]
struct CsvRecord {
    r#type: String,
    id: i32,
    palette: Option<i32>,
    comment: String,
    size: usize,
    flags: Option<u16>,
    x: Option<u16>,
    y: Option<u16>,
    width: Option<u16>,
    height: Option<u16>,
    sp_size: Option<u16>,
    tot_size: Option<u16>,
    n_sprites: Option<u16>,
    offset_x: Option<i16>,
    offset_y: Option<i16>,
    compressed_size: Option<u16>,
}

fn main() {
    let args = Cli::parse();

    let path = if args.path.is_dir() {
        args.path
    } else {
        args.path
            .parent()
            .unwrap_or_else(|| panic!("Invalid path `{}`", args.path.display()))
            .to_path_buf()
    };

    let mut sky_dnr_path = None;
    let mut sky_dsk_path = None;

    for entry in path.read_dir().expect("read_dir call failed").flatten() {
        if entry.file_name().eq_ignore_ascii_case("sky.dnr") {
            sky_dnr_path = Some(entry.path());
        }
        if entry.file_name().eq_ignore_ascii_case("sky.dsk") {
            sky_dsk_path = Some(entry.path());
        }
    }

    let sky_dnr_path = sky_dnr_path.expect("sky.dnr not found");
    let sky_dsk_path = sky_dsk_path.expect("sky.dsk not found");

    let mut sky_dnr_file = BufReader::new(
        File::open(sky_dnr_path.clone())
            .unwrap_or_else(|_| panic!("unable to open `{}`", sky_dnr_path.display())),
    );
    let mut sky_dsk_file = BufReader::new(
        File::open(sky_dsk_path.clone())
            .unwrap_or_else(|_| panic!("unable to open `{}`", sky_dsk_path.display())),
    );

    let directory = read_dinner_table(&mut sky_dnr_file).expect("error reading dnr file");

    if args.dump_csv {
        let mut wtr =
            Writer::from_path("resources.csv").expect("unable to open resources.csv for output");

        for entry in &directory {
            let data = read_entry(entry, &mut sky_dsk_file).expect("failed to read resource entry");
            let resource = read_resource(entry, data).expect("failed to read resource");

            let guessed_type = if resource.data.len() == 768 {
                "palette".to_owned()
            } else if resource.data.len() == 64000 {
                "screen".to_owned()
            } else {
                "".to_owned()
            };

            let header = resource.header;

            let csv_line = CsvRecord {
                r#type: guessed_type,
                id: entry.number.into(),
                palette: None,
                comment: "".to_owned(),
                size: resource.data.len(),
                flags: header.as_ref().map(|h| h.flags),
                x: header.as_ref().map(|h| h.x),
                y: header.as_ref().map(|h| h.y),
                width: header.as_ref().map(|h| h.width),
                height: header.as_ref().map(|h| h.height),
                sp_size: header.as_ref().map(|h| h.sp_size),
                tot_size: header.as_ref().map(|h| h.tot_size),
                n_sprites: header.as_ref().map(|h| h.n_sprites),
                offset_x: header.as_ref().map(|h| h.offset_x),
                offset_y: header.as_ref().map(|h| h.offset_y),
                compressed_size: header.as_ref().map(|h| h.compressed_size),
            };
            wtr.serialize(csv_line).expect("unable to serialize record");
        }
    }

    println!("Dumping resources to `dump/`");

    for dir in ["dump/audio", "dump/raw", "dump/screen", "dump/palette"] {
        _ = std::fs::create_dir_all(dir);
    }

    for entry in &directory {
        let data = read_entry(entry, &mut sky_dsk_file).expect("failed to read resource entry");
        dump_entry(&mut sky_dsk_file, entry).expect("failed to dump entry");

        let resource = read_resource(entry, data).expect("failed to read resource");
        if !entry.has_file_header && entry.size == 768 {
            dump_resource_as_pal(&resource).expect("failed to dump entry");
        }

        if resource.data.len() == 64000 {
            let mut pal = get_resource_by_id(entry.number + 1, &directory, &mut sky_dsk_file);
            if pal.as_ref().map_or(false, |r| r.data.len() != 768) {
                pal = get_resource_by_id(entry.number - 1, &directory, &mut sky_dsk_file);
            }
            if pal.is_some() && pal.as_ref().unwrap().data.len() != 768 {
                pal = None;
            }

            if let Some(ref pal) = pal {
                dump_screen_with_pal(entry, &pal.entry, &mut sky_dsk_file).ok();
            } else {
                dump_screen_in_grayscale(entry, &mut sky_dsk_file).ok();
            }
        } else if resource.header.map_or(false, |h| h.x & 0x8000 != 0) {
            dump_audio(entry, &mut sky_dsk_file).ok();
        }
    }
}
