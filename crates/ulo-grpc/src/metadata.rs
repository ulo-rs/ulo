use tonic::metadata::{BinaryMetadataValue, KeyAndValueRef, MetadataMap};

/// A call's metadata as `GrpcContext::from_wire` takes it: each ASCII key's values in the order
/// they arrived, a repeated key once per value, and each `-bin` value decoded from base64.
///
/// The `#[grpc_methods]` expansion calls this once per call. A value that is not visible ASCII, or
/// a `-bin` piece that is not base64, is left out.
pub fn read_metadata(metadata: &MetadataMap) -> (Vec<(String, String)>, Vec<(String, Vec<u8>)>) {
    let mut ascii = Vec::new();
    let mut binary = Vec::new();
    for entry in metadata.iter() {
        match entry {
            KeyAndValueRef::Ascii(key, value) => {
                if let Ok(value) = value.to_str() {
                    ascii.push((key.as_str().to_string(), value.to_string()));
                }
            }
            KeyAndValueRef::Binary(key, value) => {
                for bytes in decode_binary(value) {
                    binary.push((key.as_str().to_string(), bytes));
                }
            }
        }
    }
    (ascii, binary)
}

/// The values one `-bin` header carries. The specification lets a repeated key's values be
/// joined with `,` and has an implementation split a binary header on `,` before decoding, since
/// `,` is outside the base64 alphabet and the joined value decodes as nothing.
fn decode_binary(value: &BinaryMetadataValue) -> Vec<Vec<u8>> {
    value
        .as_encoded_bytes()
        .split(|b| *b == b',')
        .map(|piece| piece.trim_ascii())
        .filter_map(|piece| {
            // SAFETY: `piece` is a slice of a header value tonic already validated, trimmed of
            // spaces, so it carries no byte a header value forbids.
            let piece = unsafe {
                BinaryMetadataValue::from_shared_unchecked(bytes::Bytes::copy_from_slice(piece))
            };
            piece.to_bytes().ok().map(|bytes| bytes.to_vec())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(encoded: &str) -> BinaryMetadataValue {
        // SAFETY: base64 characters, `,` and space are all valid header-value bytes.
        unsafe {
            BinaryMetadataValue::from_shared_unchecked(bytes::Bytes::from(encoded.to_string()))
        }
    }

    #[test]
    fn a_joined_binary_value_splits_before_decoding() {
        // "YWI" is b"ab" and "Y2Q=" is b"cd", unpadded and padded.
        assert_eq!(
            decode_binary(&joined("YWI,Y2Q=")),
            [b"ab".to_vec(), b"cd".to_vec()]
        );
        assert_eq!(
            decode_binary(&joined("YWI, Y2Q=")),
            [b"ab".to_vec(), b"cd".to_vec()]
        );
    }

    #[test]
    fn a_single_binary_value_decodes_whole() {
        assert_eq!(decode_binary(&joined("YWI")), [b"ab".to_vec()]);
    }

    #[test]
    fn a_piece_that_is_not_base64_is_left_out() {
        assert_eq!(decode_binary(&joined("YWI,@@@")), [b"ab".to_vec()]);
    }

    #[test]
    fn repeated_ascii_keys_keep_their_order() {
        let mut map = MetadataMap::new();
        map.append("x-tag", "first".parse().unwrap());
        map.append("x-tag", "second".parse().unwrap());
        map.append_bin("x-trace-bin", BinaryMetadataValue::from_bytes(b"ab"));
        let (ascii, binary) = read_metadata(&map);
        assert_eq!(
            ascii,
            [
                ("x-tag".to_string(), "first".to_string()),
                ("x-tag".to_string(), "second".to_string()),
            ]
        );
        assert_eq!(binary, [("x-trace-bin".to_string(), b"ab".to_vec())]);
    }
}
