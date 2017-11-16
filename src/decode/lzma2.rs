use std::io;
use error;
use decode::decoder;
use decode::lzbuffer;
use decode::lzbuffer::LZBuffer;
use byteorder::{BigEndian, ReadBytesExt};
use decode::subbufread;
use decode::rangecoder;

pub fn decode_stream<R, W>(stream: &mut R, output: &mut W) -> error::Result<()>
where
    R: io::BufRead,
    W: io::Write,
{
    let accum = lzbuffer::LZAccumBuffer::from_stream(output);
    let mut decoder = decoder::new_accum(accum, 0, 0, 0, None);

    loop {
        let status = try!(stream.read_u8().or_else(|e| {
            Err(error::Error::LZMAError(
                format!("LZMA2 expected new status: {}", e),
            ))
        }));

        if status == 0 {
            info!("LZMA2 end of stream");
            break;
        } else if status == 1 {
            // uncompressed reset dict
            parse_uncompressed(&mut decoder, stream, true)?;
        } else if status == 2 {
            // uncompressed no reset
            parse_uncompressed(&mut decoder, stream, false)?;
        } else {
            parse_lzma(&mut decoder, stream, status)?;
        }
    }

    decoder.output.finish()?;
    Ok(())
}

fn parse_lzma<'a, R, W>(
    decoder: &mut decoder::DecoderState<lzbuffer::LZAccumBuffer<'a, W>>,
    stream: &mut R,
    status: u8,
) -> error::Result<()>
where
    R: io::BufRead,
    W: io::Write,
{
    if status & 0x80 == 0 {
        return Err(error::Error::LZMAError(format!(
            "LZMA2 invalid status: {} must be 0, 1, 2 or >= 128",
            status
        )));
    }

    let reset_dict: bool;
    let reset_state: bool;
    let reset_props: bool;
    match (status >> 5) & 0x3 {
        0 => {
            reset_dict = false;
            reset_state = false;
            reset_props = false;
        }
        1 => {
            reset_dict = false;
            reset_state = true;
            reset_props = false;
        }
        2 => {
            reset_dict = false;
            reset_state = true;
            reset_props = false;
        }
        3 => {
            reset_dict = true;
            reset_state = true;
            reset_props = true;
        }
        _ => unreachable!(),
    }

    let unpacked_size = try!(stream.read_u16::<BigEndian>().or_else(|e| {
        Err(error::Error::LZMAError(
            format!("LZMA2 expected unpacked size: {}", e),
        ))
    }));
    let unpacked_size = ((((status & 0x1F) as u64) << 16) | (unpacked_size as u64)) + 1;

    let packed_size = try!(stream.read_u16::<BigEndian>().or_else(|e| {
        Err(error::Error::LZMAError(
            format!("LZMA2 expected packed size: {}", e),
        ))
    }));
    let packed_size = (packed_size as usize) + 1;

    info!(
        "LZMA2 compressed block {{ unpacked_size: {}, packed_size: {}, reset_dict: {}, reset_state: {}, reset_props: {} }}",
        unpacked_size,
        packed_size,
        reset_dict,
        reset_state,
        reset_props
    );

    if reset_dict {
        decoder.output.reset()?;
    }

    if reset_state {
        let lc: u32;
        let lp: u32;
        let mut pb: u32;

        if reset_props {
            let props = try!(stream.read_u8().or_else(|e| {
                Err(error::Error::LZMAError(
                    format!("LZMA2 expected new properties: {}", e),
                ))
            }));

            pb = props as u32;
            if pb >= 225 {
                return Err(error::Error::LZMAError(
                    format!("LZMA2 invalid properties: {} must be < 225", pb),
                ));
            }

            lc = pb % 9;
            pb /= 9;
            lp = pb % 5;
            pb /= 5;

            if lc + lp > 4 {
                return Err(error::Error::LZMAError(format!(
                    "LZMA2 invalid properties: lc + lp ({} + {}) must be <= 4",
                    lc,
                    lp
                )));
            }

            info!("Properties {{ lc: {}, lp: {}, pb: {} }}", lc, lp, pb);
        } else {
            lc = decoder.lc;
            lp = decoder.lp;
            pb = decoder.pb;
        }

        decoder.reset_state(lc, lp, pb);
    }

    decoder.set_unpacked_size(Some(unpacked_size));

    let mut subbufread = subbufread::SubBufRead::new(stream, packed_size);
    let mut rangecoder = try!(rangecoder::RangeDecoder::new(&mut subbufread).or_else(
        |e| {
            Err(error::Error::LZMAError(
                format!("LZMA stream too short: {}", e),
            ))
        },
    ));
    decoder.process(&mut rangecoder)
}

fn parse_uncompressed<'a, R, W>(
    decoder: &mut decoder::DecoderState<lzbuffer::LZAccumBuffer<'a, W>>,
    stream: &mut R,
    reset_dict: bool,
) -> error::Result<()>
where
    R: io::BufRead,
    W: io::Write,
{
    let unpacked_size = try!(stream.read_u16::<BigEndian>().or_else(|e| {
        Err(error::Error::LZMAError(
            format!("LZMA2 expected unpacked size: {}", e),
        ))
    }));
    let unpacked_size = (unpacked_size as usize) + 1;

    info!(
        "LZMA2 uncompressed block {{ unpacked_size: {}, reset_dict: {} }}",
        unpacked_size,
        reset_dict
    );

    if reset_dict {
        decoder.output.reset()?;
    }

    let mut buf = vec![0; unpacked_size];
    try!(stream.read_exact(buf.as_mut_slice()).or_else(|e| {
        Err(error::Error::LZMAError(format!(
            "LZMA2 expected {} uncompressed bytes: {}",
            unpacked_size,
            e
        )))
    }));
    decoder.output.append_bytes(buf.as_slice());

    Ok(())
}