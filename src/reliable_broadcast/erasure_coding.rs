use atlas_common::serialization_helper::SerMsg;
use reed_solomon_erasure::galois_8::ReedSolomon;
use thiserror::Error;

/// `(n-2f, n)` erasure coding parameters (Algorithm 5): `data_shards = n-2f`
/// symbols suffice to reconstruct the original value, dispersed across `n`
/// total shards (`parity_shards = 2f` redundant ones).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ErasureParams {
    data_shards: usize,
    parity_shards: usize,
}

impl ErasureParams {
    pub(crate) fn for_quorum(n: usize, f: usize) -> Self {
        Self {
            data_shards: n - 2 * f,
            parity_shards: 2 * f,
        }
    }

    pub(crate) fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }

    pub(crate) fn data_shards(&self) -> usize {
        self.data_shards
    }
}

const LEN_PREFIX_BYTES: usize = 8;

fn ceil_div(a: usize, b: usize) -> usize {
    a.div_ceil(b)
}

/// Serializes `value`, frames it with an 8-byte little-endian length
/// prefix, zero-pads it to a multiple of `data_shards`, and splits/encodes
/// it into exactly `total_shards()` byte shards of equal length.
pub(crate) fn encode<RQ: SerMsg>(
    value: &RQ,
    params: &ErasureParams,
) -> Result<Vec<Vec<u8>>, ErasureCodingError> {
    let payload = bincode::serde::encode_to_vec(value, bincode::config::standard())?;

    let mut framed = Vec::with_capacity(LEN_PREFIX_BYTES + payload.len());
    framed.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    framed.extend_from_slice(&payload);

    let shard_len = ceil_div(framed.len(), params.data_shards).max(1);
    framed.resize(shard_len * params.data_shards, 0);

    let mut shards: Vec<Vec<u8>> = framed.chunks(shard_len).map(|c| c.to_vec()).collect();

    if params.parity_shards == 0 {
        // No redundancy requested (f == 0): nothing to encode, the data
        // shards themselves are the complete shard set.
        return Ok(shards);
    }

    shards.resize(params.total_shards(), vec![0u8; shard_len]);

    let rs = ReedSolomon::new(params.data_shards, params.parity_shards)?;
    rs.encode(&mut shards)?;

    Ok(shards)
}

/// Fills in every `None` shard (both data and parity positions) given at
/// least `data_shards` are `Some`. Deliberately reconstructs *all* `n`
/// shards, not just the data ones, so a subsequent Merkle-root recheck
/// covers positions a data-only reconstruct would never touch.
pub(crate) fn reconstruct_all(
    shards: &mut [Option<Vec<u8>>],
    params: &ErasureParams,
) -> Result<(), ErasureCodingError> {
    if params.parity_shards == 0 {
        return if shards.iter().all(Option::is_some) {
            Ok(())
        } else {
            Err(ErasureCodingError::InsufficientShards)
        };
    }

    let rs = ReedSolomon::new(params.data_shards, params.parity_shards)?;
    rs.reconstruct(shards)?;

    Ok(())
}

/// Concatenates the first `data_shards` shards, strips the length prefix +
/// padding, and deserializes the original value.
pub(crate) fn decode<RQ: SerMsg>(
    data_shards: &[Vec<u8>],
    params: &ErasureParams,
) -> Result<RQ, ErasureCodingError> {
    let mut framed = Vec::new();

    for shard in &data_shards[..params.data_shards] {
        framed.extend_from_slice(shard);
    }

    if framed.len() < LEN_PREFIX_BYTES {
        return Err(ErasureCodingError::InsufficientShards);
    }

    let mut len_bytes = [0u8; LEN_PREFIX_BYTES];
    len_bytes.copy_from_slice(&framed[..LEN_PREFIX_BYTES]);
    let payload_len = u64::from_le_bytes(len_bytes) as usize;

    let payload_end = LEN_PREFIX_BYTES + payload_len;
    if framed.len() < payload_end {
        return Err(ErasureCodingError::InsufficientShards);
    }

    let (value, _) = bincode::serde::decode_from_slice::<RQ, _>(
        &framed[LEN_PREFIX_BYTES..payload_end],
        bincode::config::standard(),
    )?;

    Ok(value)
}

#[derive(Debug, Error)]
pub(crate) enum ErasureCodingError {
    #[error("Reed-Solomon error: {0}")]
    Rs(#[from] reed_solomon_erasure::Error),
    #[error("Failed to serialize value for erasure coding: {0}")]
    Encode(#[from] bincode::error::EncodeError),
    #[error("Failed to deserialize value after erasure decoding: {0}")]
    Decode(#[from] bincode::error::DecodeError),
    #[error("Not enough shards to reconstruct")]
    InsufficientShards,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestValue(Vec<u8>);

    #[test]
    fn test_encode_decode_round_trip() {
        let params = ErasureParams::for_quorum(7, 2);
        let value = TestValue(b"hello erasure coded world".to_vec());

        let shards = encode(&value, &params).unwrap();
        assert_eq!(shards.len(), params.total_shards());

        let decoded: TestValue = decode(&shards, &params).unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn test_reconstruct_from_data_shards_only() {
        let params = ErasureParams::for_quorum(7, 2);
        let value = TestValue(vec![42; 500]);

        let shards = encode(&value, &params).unwrap();

        let mut partial: Vec<Option<Vec<u8>>> = shards
            .iter()
            .take(params.data_shards())
            .cloned()
            .map(Some)
            .chain(std::iter::repeat_n(
                None,
                params.total_shards() - params.data_shards(),
            ))
            .collect();

        reconstruct_all(&mut partial, &params).unwrap();

        let reconstructed: Vec<Vec<u8>> = partial.into_iter().map(Option::unwrap).collect();
        assert_eq!(reconstructed, shards);
    }

    #[test]
    fn test_reconstruct_from_parity_shards() {
        let params = ErasureParams::for_quorum(7, 2);
        let value = TestValue(vec![7; 123]);

        let shards = encode(&value, &params).unwrap();

        // Keep only the last `data_shards` shards (mostly/entirely parity).
        let mut partial: Vec<Option<Vec<u8>>> = vec![None; params.total_shards()];
        for i in (params.total_shards() - params.data_shards())..params.total_shards() {
            partial[i] = Some(shards[i].clone());
        }

        reconstruct_all(&mut partial, &params).unwrap();

        let reconstructed: Vec<Vec<u8>> = partial.into_iter().map(Option::unwrap).collect();
        assert_eq!(reconstructed, shards);

        let decoded: TestValue = decode(&reconstructed, &params).unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn test_f_zero_degenerate_path() {
        let params = ErasureParams::for_quorum(4, 0);
        assert_eq!(params.parity_shards, 0);

        let value = TestValue(b"no redundancy".to_vec());
        let shards = encode(&value, &params).unwrap();
        assert_eq!(shards.len(), params.total_shards());

        let decoded: TestValue = decode(&shards, &params).unwrap();
        assert_eq!(decoded, value);
    }
}
