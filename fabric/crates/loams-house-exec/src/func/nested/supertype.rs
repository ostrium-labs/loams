//! ClickHouse's least supertype for the composite-type functions: the type of
//! `[a, b]`, of `map`'s keys and values, of `arrayConcat` and of `has`'s
//! comparison. The rules are plan HE1 Task 24's
//! (`loams_house_sql::coerce::rules::least_supertype`, measured on the
//! oracle); maps, which that function does not take, meet key by key and
//! value by value (`getLeastSupertype`, `src/DataTypes/getLeastSupertype.cpp`,
//! Apache-2.0).

use loams_house_sql::coerce::rules::least_supertype;
use loams_house_types::{ChError, ChType};

/// The least supertype of `types` (`Nothing` when there are none).
pub fn supertype(types: &[ChType]) -> Result<ChType, ChError> {
    if !types.is_empty() && types.iter().all(|t| matches!(t, ChType::Map(..))) {
        let (mut keys, mut values) = (Vec::new(), Vec::new());
        for t in types {
            if let ChType::Map(k, v) = t {
                keys.push((**k).clone());
                values.push((**v).clone());
            }
        }
        return Ok(ChType::Map(
            Box::new(supertype(&keys)?),
            Box::new(supertype(&values)?),
        ));
    }
    least_supertype(types)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use loams_house_types::parse;

    fn st(types: &[&str]) -> Result<String, ChError> {
        let types: Vec<ChType> = types.iter().map(|t| parse(t).unwrap()).collect();
        supertype(&types).map(|t| t.to_string())
    }

    #[test]
    fn maps_meet_by_keys_and_values() {
        assert_eq!(st(&[]).unwrap(), "Nothing");
        assert_eq!(st(&["UInt8", "UInt16"]).unwrap(), "UInt16");
        assert_eq!(
            st(&["Map(String, UInt8)", "Map(String, UInt32)"]).unwrap(),
            "Map(String, UInt32)"
        );
    }
}
