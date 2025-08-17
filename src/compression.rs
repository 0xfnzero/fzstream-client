// Compression utilities for the Fast Stream Client SDK
use anyhow::Result;
use fzstream_common::CompressionType;

/// Compress data using the specified compression algorithm
pub fn compress_data(data: &[u8], compression_type: &CompressionType) -> Result<Vec<u8>> {
    match compression_type {
        CompressionType::None => Ok(data.to_vec()),
        CompressionType::LZ4 => {
            let compressed = lz4::block::compress(data, None, false)?;
            Ok(compressed)
        }
        CompressionType::Zstd => {
            let compressed = zstd::encode_all(&data[..], 1)?;
            Ok(compressed)
        }
    }
}

/// Decompress data using the specified compression algorithm
pub fn decompress_data(data: &[u8], compression_type: &CompressionType) -> Result<Vec<u8>> {
    match compression_type {
        CompressionType::None => Ok(data.to_vec()),
        CompressionType::LZ4 => {
            let decompressed = lz4::block::decompress(data, Some((data.len() * 4) as i32))?;
            Ok(decompressed)
        }
        CompressionType::Zstd => {
            let decompressed = zstd::decode_all(&data[..])?;
            Ok(decompressed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lz4_compression() {
        let data = b"Hello, World! This is a test string for compression.";
        
        let compressed = compress_data(data, &CompressionType::LZ4).unwrap();
        let decompressed = decompress_data(&compressed, &CompressionType::LZ4).unwrap();
        
        assert_eq!(data, decompressed.as_slice());
    }

    #[test]
    fn test_zstd_compression() {
        let data = b"Hello, World! This is a test string for compression.";
        
        let compressed = compress_data(data, &CompressionType::Zstd).unwrap();
        let decompressed = decompress_data(&compressed, &CompressionType::Zstd).unwrap();
        
        assert_eq!(data, decompressed.as_slice());
    }

    #[test]
    fn test_no_compression() {
        let data = b"Hello, World!";
        
        let compressed = compress_data(data, &CompressionType::None).unwrap();
        let decompressed = decompress_data(&compressed, &CompressionType::None).unwrap();
        
        assert_eq!(data, decompressed.as_slice());
        assert_eq!(data, compressed.as_slice());
    }
}