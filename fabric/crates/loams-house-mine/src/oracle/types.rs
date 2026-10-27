//! ClickHouse type names, read only as far as the normaliser needs: which
//! values are integers, floats or decimals, and the element types of `Array`,
//! `Tuple` and `Map`. The type name itself is always compared verbatim
//! (`normaliser_never_hides_row_count_or_types`); this reading only decides how
//! a value of that type is put in canonical form.

/// What the normaliser knows about a column's type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// `Int8` … `Int256`, `UInt8` … `UInt256`: written as a JSON number or, for
    /// 64 bits and wider, as a quoted string.
    Int,
    /// `Float32` and `BFloat16`.
    Float32,
    /// `Float64`.
    Float64,
    /// `Decimal(P, S)` and its sized spellings.
    Decimal,
    /// `Array(T)`.
    Array(Box<Shape>),
    /// `Tuple(T1, T2, …)`, named or not.
    Tuple(Vec<Shape>),
    /// `Map(K, V)`.
    Map(Box<Shape>),
    /// Anything else: compared as the JSON value it is.
    Other,
}

/// Reads a type name. `Nullable(…)` and `LowCardinality(…)` are looked through:
/// they change the type name (compared verbatim) but not how a value is written.
pub fn shape(name: &str) -> Shape {
    let name = name.trim();
    let (head, args) = match name.find('(') {
        Some(open) if name.ends_with(')') => (&name[..open], Some(&name[open + 1..name.len() - 1])),
        _ => (name, None),
    };
    match (head.trim(), args) {
        ("Nullable" | "LowCardinality", Some(inner)) => shape(inner),
        ("Float64", _) => Shape::Float64,
        ("Float32" | "BFloat16", _) => Shape::Float32,
        ("Decimal" | "Decimal32" | "Decimal64" | "Decimal128" | "Decimal256", _) => Shape::Decimal,
        ("Array", Some(inner)) => Shape::Array(Box::new(shape(inner))),
        ("Tuple", Some(inner)) => Shape::Tuple(
            split_top_level(inner)
                .into_iter()
                .map(|element| shape(unnamed(element)))
                .collect(),
        ),
        ("Map", Some(inner)) => match split_top_level(inner).as_slice() {
            [_, value] => Shape::Map(Box::new(shape(value))),
            _ => Shape::Other,
        },
        (head, None) if is_int(head) => Shape::Int,
        _ => Shape::Other,
    }
}

fn is_int(head: &str) -> bool {
    let digits = head
        .strip_prefix("UInt")
        .or_else(|| head.strip_prefix("Int"));
    matches!(digits, Some("8" | "16" | "32" | "64" | "128" | "256"))
}

/// A named tuple element (`a Int32`) without its name. A type name has no space
/// outside its parentheses, so a space at depth 0 ends the element's name.
fn unnamed(element: &str) -> &str {
    let element = element.trim();
    let mut depth = 0usize;
    let mut quoted = false;
    for (i, c) in element.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth = depth.saturating_sub(1),
            ' ' if !quoted && depth == 0 => return element[i + 1..].trim(),
            _ => {}
        }
    }
    element
}

/// Splits `a, b(c, d), 'e,f'` at the commas outside parentheses and quotes.
fn split_top_level(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in args.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth = depth.saturating_sub(1),
            ',' if !quoted && depth == 0 => {
                parts.push(args[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_of_common_types() {
        assert_eq!(shape("UInt64"), Shape::Int);
        assert_eq!(shape("Nullable(Int32)"), Shape::Int);
        assert_eq!(shape("LowCardinality(Nullable(String))"), Shape::Other);
        assert_eq!(shape("Float32"), Shape::Float32);
        assert_eq!(shape("Decimal(18, 4)"), Shape::Decimal);
        assert_eq!(shape("DateTime64(3, 'UTC')"), Shape::Other);
        assert_eq!(shape("Interval"), Shape::Other);
        assert_eq!(
            shape("Array(Float64)"),
            Shape::Array(Box::new(Shape::Float64))
        );
        assert_eq!(
            shape("Tuple(a Int32, b Array(Float32), String)"),
            Shape::Tuple(vec![
                Shape::Int,
                Shape::Array(Box::new(Shape::Float32)),
                Shape::Other
            ])
        );
        assert_eq!(
            shape("Map(LowCardinality(String), UInt64)"),
            Shape::Map(Box::new(Shape::Int))
        );
    }
}
