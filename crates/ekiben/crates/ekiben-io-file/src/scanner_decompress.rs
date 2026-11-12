//! `decompress` scanner (`internal/impl/pure/scanner_decompress.go`): decompresses the stream with
//! `algorithm` before the child scanner (`into`, default `to_the_end`) reads it.

use std::io::Read;

use ekiben_core::CoreError;

/// Decompressed bytes one stream may grow to: past it the read fails (zip bomb guard).
pub const MAX_DECOMPRESSED_BYTES: u64 = 128 * 1024 * 1024;

/// Bento's decompression algorithms (`internal/impl/pure/algorithms.go`, `extended/zstd.go`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    Gzip,
    Pgzip,
    Zlib,
    Flate,
    Bzip2,
    Lz4,
    Snappy,
    Zstd,
}

impl Algorithm {
    /// Bento's `strToDecompressReader`, with its error text for an unknown name.
    pub fn parse(name: &str) -> Result<Self, CoreError> {
        Ok(match name {
            "gzip" => Self::Gzip,
            "pgzip" => Self::Pgzip,
            "zlib" => Self::Zlib,
            "flate" => Self::Flate,
            "bzip2" => Self::Bzip2,
            "lz4" => Self::Lz4,
            "snappy" => Self::Snappy,
            "zstd" => Self::Zstd,
            other => {
                return Err(CoreError::config(format!(
                    "compression type not recognised: {other}"
                )))
            }
        })
    }

    /// The whole decompressed stream; concatenated gzip, bzip2, lz4 and zstd members read on as Go's readers do.
    pub fn decompress(self, raw: &[u8]) -> Result<Vec<u8>, CoreError> {
        let reader: Box<dyn Read + '_> = match self {
            Self::Gzip | Self::Pgzip => Box::new(flate2::read::MultiGzDecoder::new(raw)),
            Self::Zlib => Box::new(flate2::read::ZlibDecoder::new(raw)),
            Self::Flate => Box::new(flate2::read::DeflateDecoder::new(raw)),
            Self::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(raw)),
            Self::Lz4 => Box::new(lz4_flex::frame::FrameDecoder::new(raw)),
            Self::Snappy => Box::new(snap::read::FrameDecoder::new(raw)),
            Self::Zstd => Box::new(
                zstd::stream::read::Decoder::new(raw).map_err(|e| CoreError::other(e.to_string()))?,
            ),
        };
        let mut out = Vec::new();
        reader
            .take(MAX_DECOMPRESSED_BYTES + 1)
            .read_to_end(&mut out)
            .map_err(|e| CoreError::other(e.to_string()))?;
        if out.len() as u64 > MAX_DECOMPRESSED_BYTES {
            return Err(CoreError::other(format!(
                "decompressed data exceeds safety limit ({MAX_DECOMPRESSED_BYTES} bytes)"
            )));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Bento `TestDecompressScannerSuite`: gzip into `lines` with `custom_delimiter: X` (the
    /// delimiter split is the lines scanner's; this checks the decompressed stream it reads).
    #[test]
    fn test_decompress_scanner_suite() {
        let input = hex(
            "1f8b080000096e8800ff001e00e1ff68656c6c6f58776f726c64587468697358697358636f6d7072657373656403009104d92d1e000000",
        );
        let out = Algorithm::Gzip.decompress(&input).expect("gzip");
        let parts: Vec<&[u8]> = out.split(|b| *b == b'X').collect();
        assert_eq!(
            parts,
            [&b"hello"[..], b"world", b"this", b"is", b"compressed"]
        );
    }

    #[test]
    fn every_algorithm_reads_its_own_stream() {
        let body = b"hello world".repeat(100);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&body).expect("gz");
        let gz = gz.finish().expect("gz");
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&body).expect("zlib");
        let z = z.finish().expect("zlib");
        let mut fl = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        fl.write_all(&body).expect("flate");
        let fl = fl.finish().expect("flate");
        let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        bz.write_all(&body).expect("bzip2");
        let bz = bz.finish().expect("bzip2");
        let mut lz = lz4_flex::frame::FrameEncoder::new(Vec::new());
        lz.write_all(&body).expect("lz4");
        let lz = lz.finish().expect("lz4");
        let mut sn = snap::write::FrameEncoder::new(Vec::new());
        sn.write_all(&body).expect("snappy");
        let sn = sn.into_inner().expect("snappy");
        let zs = zstd::stream::encode_all(&body[..], 0).expect("zstd");
        for (alg, stream) in [
            (Algorithm::Gzip, &gz),
            (Algorithm::Pgzip, &gz),
            (Algorithm::Zlib, &z),
            (Algorithm::Flate, &fl),
            (Algorithm::Bzip2, &bz),
            (Algorithm::Lz4, &lz),
            (Algorithm::Snappy, &sn),
            (Algorithm::Zstd, &zs),
        ] {
            assert_eq!(alg.decompress(stream).expect("decompressed"), body, "{alg:?}");
        }
        let mut two = gz.clone();
        two.extend_from_slice(&gz);
        assert_eq!(Algorithm::Gzip.decompress(&two).expect("two members").len(), body.len() * 2);
    }

    #[test]
    fn an_unknown_algorithm_names_itself() {
        let err = Algorithm::parse("rar").expect_err("unknown");
        assert!(err.to_string().contains("compression type not recognised: rar"), "{err}");
    }

    #[test]
    fn a_zip_bomb_stops_at_the_cap() {
        let zeros = vec![0u8; 1 << 20];
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        for _ in 0..(MAX_DECOMPRESSED_BYTES / (1 << 20) + 1) {
            gz.write_all(&zeros).expect("gz");
        }
        let bomb = gz.finish().expect("gz");
        assert!(bomb.len() < 1 << 20, "a small stream: {}", bomb.len());
        let err = Algorithm::Gzip.decompress(&bomb).expect_err("capped");
        assert!(err.to_string().contains("exceeds safety limit"), "{err}");
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }
}
