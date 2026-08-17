//! [`OrderedMap`]: deserializes a TOML table preserving its declaration
//! order — used for `[backs]` and `[sounds]`, which the public API exposes
//! as `Vec<(K, V)>` rather than a sorted map, so a
//! theme author's declaration order survives into [`crate::Manifest`].

use core::fmt;
use core::marker::PhantomData;

use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};

/// A table deserialized as a `Vec` of `(key, value)` pairs in the order
/// they appeared in the source document, rather than a resorted map.
#[derive(Debug)]
pub(crate) struct OrderedMap<V>(pub(crate) Vec<(String, V)>);

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

/// Visits a TOML table, collecting its entries in declaration order. A named
/// (rather than function-local) type so its `expecting` message is directly
/// unit-testable via [`serde::de::Expected`].
struct MapVisitor<V>(PhantomData<V>);

impl<'de, V: Deserialize<'de>> Visitor<'de> for MapVisitor<V> {
    type Value = OrderedMap<V>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a table")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }
        Ok(OrderedMap(entries))
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for OrderedMap<V> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(MapVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn preserves_declaration_order_over_alphabetical_order() {
        // Alphabetically this would be apple, plain, robot — deliberately
        // declared out of alphabetical order so a naive `BTreeMap` (which
        // sorts) would fail this test but an order-preserving map passes.
        let toml = "robot = \"r\"\nplain = \"p\"\napple = \"a\"\n";
        let map: OrderedMap<String> = toml::from_str(toml).unwrap();
        let keys: Vec<&str> = map.0.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["robot", "plain", "apple"]);
    }

    #[test]
    fn deserializes_an_empty_table() {
        let map: OrderedMap<String> = toml::from_str("").unwrap();
        assert!(map.0.is_empty());
    }

    #[test]
    fn a_non_table_value_is_rejected_and_names_the_expected_shape() {
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            #[allow(dead_code)]
            backs: OrderedMap<String>,
        }

        // Not a table: exercises the Visitor's `expecting` message, which
        // only fires when serde reports a type mismatch.
        let error = toml::from_str::<Wrapper>("backs = \"not a table\"").unwrap_err();
        assert!(error.to_string().contains("a table"), "{error}");
    }

    #[test]
    fn deserializes_typed_values() {
        let toml = "a = 1\nb = 2\n";
        let map: OrderedMap<i64> = toml::from_str(toml).unwrap();
        assert_eq!(map.0, vec![("a".to_owned(), 1), ("b".to_owned(), 2)]);
    }

    #[test]
    fn default_is_empty() {
        let map: OrderedMap<String> = OrderedMap::default();
        assert!(map.0.is_empty());
    }

    #[test]
    fn map_visitor_expecting_message_is_exactly_a_table() {
        // Reads `expecting`'s actual formatted output directly, rather than
        // relying on it surfacing unchanged through a toml parse error (the
        // `toml` crate's own wording for this case can coincide with ours).
        let visitor: MapVisitor<String> = MapVisitor(PhantomData);
        let expected: &dyn serde::de::Expected = &visitor;
        assert_eq!(expected.to_string(), "a table");
    }
}
