//! A fixed integer hash for seeded, repeatable randomness.

/// The splitmix64 finaliser: spreads every input bit over the whole output.
/// Pinned so a seed gives the same result on every build.
pub fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_pinned() {
        assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
        assert_ne!(splitmix64(1), splitmix64(2));
    }
}
