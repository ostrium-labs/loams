//! The least supertype of several ClickHouse types, for `if`, `multiIf`,
//! `coalesce`, `greatest` and the rest: Task 24's rules
//! (`loams_house_sql::coerce::rules::least_supertype`, a port of
//! ClickHouse's `getLeastSupertype`), so every expression in the engine takes
//! one supertype (R33.5).

pub use loams_house_sql::coerce::rules::least_supertype;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use loams_house_types::{ChType, parse};

    fn s(types: &[&str]) -> String {
        let types: Vec<ChType> = types.iter().map(|t| parse(t).unwrap()).collect();
        least_supertype(&types).unwrap().to_string()
    }

    #[test]
    fn the_conditionals_supertypes() {
        assert_eq!(s(&["UInt8", "UInt16"]), "UInt16");
        assert_eq!(s(&["UInt8", "Int8"]), "Int16");
        assert_eq!(s(&["Nullable(Nothing)", "UInt8"]), "Nullable(UInt8)");
        assert_eq!(s(&["String", "String"]), "String");
    }
}
