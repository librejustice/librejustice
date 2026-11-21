//! A JSON document as an element tree that gives it back, written into stored shapes: a string `k`, empty for `""`,
//! an integer `k#`, an object `k` with its leading `@` members as attributes or `k{}`, an array `k[]` of items named `k`.

use std::fmt;

use serde::de::{DeserializeSeed, Deserializer, Error as _, MapAccess, SeqAccess, Visitor};

use crate::tree::{Body, Node};
use crate::Error;

/// The shape's own characters, the path separator and this module's suffixes.
const RESERVED: &[char] = &[
    '@', '$', '%', '&', '(', ')', ',', '/', '#', '[', ']', '{', '}',
];

pub fn parse(json: &str) -> Result<Node, Error> {
    let mut de = serde_json::Deserializer::from_str(json);
    let root = (&mut de)
        .deserialize_map(Root)
        .map_err(|e| Error::new(format!("JSON: {e}")))?;
    de.end().map_err(|e| Error::new(format!("JSON: {e}")))?;
    Ok(root)
}

fn checked<E: serde::de::Error>(name: &str) -> Result<String, E> {
    if name.is_empty() || name.contains(RESERVED) {
        return Err(E::custom(format!("key {name:?} cannot name an element")));
    }
    Ok(name.to_string())
}

struct Root;

impl<'de> Visitor<'de> for Root {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object of one member")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let key: String = map
            .next_key()?
            .ok_or_else(|| A::Error::custom("empty document"))?;
        let root = map.next_value_seed(Member(checked(&key)?))?;
        if map.next_key::<String>()?.is_some() {
            return Err(A::Error::custom("more than one root member"));
        }
        Ok(root)
    }
}

struct Member(String);

impl<'de> DeserializeSeed<'de> for Member {
    type Value = Node;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Node, D::Error> {
        de.deserialize_any(self)
    }
}

fn leaf(name: String, body: Body) -> Node {
    Node {
        name,
        attrs: Vec::new(),
        body,
    }
}

impl<'de> Visitor<'de> for Member {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a string, an integer, an object or an array at {}",
            self.0
        )
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Node, E> {
        Ok(leaf(
            self.0,
            if v.is_empty() {
                Body::Empty
            } else {
                Body::Text(v.to_string())
            },
        ))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Node, E> {
        Ok(leaf(format!("{}#", self.0), Body::Text(v.to_string())))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Node, E> {
        Ok(leaf(format!("{}#", self.0), Body::Text(v.to_string())))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut attrs = Vec::new();
        let mut children = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            match key.strip_prefix('@') {
                Some(attr) if children.is_empty() => {
                    attrs.push((checked(attr)?, map.next_value::<String>()?));
                }
                Some(_) => {
                    return Err(A::Error::custom(format!(
                        "{key} follows a member in {}",
                        self.0
                    )))
                }
                None => children.push(map.next_value_seed(Member(checked(&key)?))?),
            }
        }
        Ok(match (attrs.is_empty(), children.is_empty()) {
            (true, true) => leaf(format!("{}{{}}", self.0), Body::Empty),
            (_, true) => Node {
                name: self.0,
                attrs,
                body: Body::Empty,
            },
            _ => Node {
                name: self.0,
                attrs,
                body: Body::Children(children),
            },
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Member(self.0.clone()))? {
            items.push(item);
        }
        Ok(leaf(
            format!("{}[]", self.0),
            if items.is_empty() {
                Body::Empty
            } else {
                Body::Children(items)
            },
        ))
    }
}
