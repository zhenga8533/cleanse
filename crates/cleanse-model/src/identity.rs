use serde::{Deserialize, Serialize};

/// Identifies a file or directory independently of its path, so Cleanse can tell whether the
/// item it scanned is still the one at that path when it comes to remove it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    /// The device (Unix) or volume serial number (Windows).
    pub volume: u64,
    /// The inode (Unix) or file ID (Windows, 128 bits wide on ReFS). Written as a decimal
    /// string: serde can't read 128-bit numbers back inside tagged enums, such as journal
    /// records, and JavaScript can't hold them.
    #[serde(with = "decimal")]
    pub file: u128,
}

mod decimal {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_ids_are_written_as_decimal_strings_and_read_back() {
        let identity = FileIdentity {
            volume: 7,
            file: u128::from(u64::MAX) + 1,
        };

        let json = serde_json::to_value(identity).unwrap();

        assert_eq!(
            json,
            serde_json::json!({ "volume": 7, "file": "18446744073709551616" })
        );
        assert_eq!(
            serde_json::from_value::<FileIdentity>(json).unwrap(),
            identity
        );
    }
}
