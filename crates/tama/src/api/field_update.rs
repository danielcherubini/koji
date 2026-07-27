//! Tri-state for PATCH bodies: field absent (leave unchanged), explicit null
//! (clear), or a value (set). Replaces `Option<Option<T>>` at API boundaries.

use serde::{Deserialize, Deserializer};

/// PATCH tri-state. `#[serde(default)]` on the field gives `Unchanged` when absent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FieldUpdate<T> {
    #[default]
    Unchanged,
    Clear,
    Set(T),
}

impl<'de, T> Deserialize<'de> for FieldUpdate<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(v) => Self::Set(v),
            None => Self::Clear,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct TestDto {
        #[serde(default)]
        name: FieldUpdate<String>,
    }

    /// Missing key → `Unchanged` (default).
    #[test]
    fn test_missing_key_is_unchanged() {
        let dto: TestDto = serde_json::from_str("{}").unwrap();
        assert!(matches!(dto.name, FieldUpdate::Unchanged));
    }

    /// Explicit `null` → `Clear`.
    #[test]
    fn test_explicit_null_is_clear() {
        let dto: TestDto = serde_json::from_str(r#"{"name": null}"#).unwrap();
        assert!(matches!(dto.name, FieldUpdate::Clear));
    }

    /// Value → `Set`.
    #[test]
    fn test_value_is_set() {
        let dto: TestDto = serde_json::from_str(r#"{"name": "hello"}"#).unwrap();
        assert!(matches!(dto.name, FieldUpdate::Set(ref s) if s == "hello"));
    }

    /// `FieldUpdate<u16>` missing key → `Unchanged`.
    #[test]
    fn test_field_update_u16_missing() {
        #[derive(Debug, Deserialize)]
        struct PortDto {
            #[serde(default)]
            port: FieldUpdate<u16>,
        }

        let dto: PortDto = serde_json::from_str("{}").unwrap();
        assert!(matches!(dto.port, FieldUpdate::Unchanged));
    }

    /// `FieldUpdate<u16>` explicit null → `Clear`.
    #[test]
    fn test_field_update_u16_null() {
        #[derive(Debug, Deserialize)]
        struct PortDto {
            #[serde(default)]
            port: FieldUpdate<u16>,
        }

        let dto: PortDto = serde_json::from_str(r#"{"port": null}"#).unwrap();
        assert!(matches!(dto.port, FieldUpdate::Clear));
    }

    /// `FieldUpdate<u16>` value → `Set`.
    #[test]
    fn test_field_update_u16_value() {
        #[derive(Debug, Deserialize)]
        struct PortDto {
            #[serde(default)]
            port: FieldUpdate<u16>,
        }

        let dto: PortDto = serde_json::from_str(r#"{"port": 8080}"#).unwrap();
        assert!(matches!(dto.port, FieldUpdate::Set(8080)));
    }
}
