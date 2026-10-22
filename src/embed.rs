use std::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
    ser::SerializeMap,
};
use serde_json::{Map, Value};

use crate::{Result, Text, Usage, openai::OaUsage};

/// An embeddings request (OpenAI `/embeddings` format).
#[derive(Clone, Debug)]
pub struct EmbedRequest {
    /// Model id.
    pub model: Text,
    /// Texts to embed.
    pub input: Vec<Text>,
    /// Output dimensions, for models that support shortening.
    pub dimensions: Option<u32>,
    /// Extra top-level JSON fields, sent as-is.
    pub extra: Map<String, Value>,
}

impl EmbedRequest {
    /// New request for `model`.
    pub fn new(model: impl Into<Text>) -> Self {
        Self { model: model.into(), input: Vec::new(), dimensions: None, extra: Map::new() }
    }
    /// Add one text.
    pub fn input(mut self, text: impl Into<Text>) -> Self {
        self.input.push(text.into());
        self
    }
    /// Add many texts.
    pub fn inputs<T: Into<Text>>(mut self, texts: impl IntoIterator<Item = T>) -> Self {
        self.input.extend(texts.into_iter().map(Into::into));
        self
    }
    /// Set output dimensions.
    pub fn dimensions(mut self, n: u32) -> Self {
        self.dimensions = Some(n);
        self
    }
    /// Set any other top-level field.
    pub fn extra(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

pub(crate) struct Body<'a>(pub &'a EmbedRequest);

impl Serialize for Body<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let r = self.0;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("model", &r.model)?;
        m.serialize_entry("input", &r.input)?;
        if let Some(d) = r.dimensions {
            m.serialize_entry("dimensions", &d)?;
        }
        for (k, v) in &r.extra {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// Embedding vectors, stored in one contiguous buffer (one allocation for the whole batch).
#[derive(Clone, Debug, Default)]
pub struct Embeddings {
    data: Vec<f32>,
    dim: usize,
    /// Model that answered.
    pub model: String,
    /// Token counts.
    pub usage: Usage,
}

impl Embeddings {
    /// Number of vectors.
    pub fn len(&self) -> usize {
        self.data.len().checked_div(self.dim).unwrap_or(0)
    }
    /// No vectors.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    /// Length of each vector.
    pub fn dim(&self) -> usize {
        self.dim
    }
    /// Vector `i`, in input order.
    pub fn get(&self, i: usize) -> Option<&[f32]> {
        self.data.get(i * self.dim..(i + 1) * self.dim).filter(|_| self.dim > 0)
    }
    /// All vectors, in input order.
    pub fn iter(&self) -> std::slice::ChunksExact<'_, f32> {
        self.data.chunks_exact(self.dim.max(1))
    }
    /// All vectors back to back.
    pub fn as_flat(&self) -> &[f32] {
        &self.data
    }
    /// Take the flat buffer.
    pub fn into_flat(self) -> Vec<f32> {
        self.data
    }
}

pub(crate) fn parse(body: &[u8]) -> Result<Embeddings> {
    let mut acc = Acc::default();
    let mut de = serde_json::Deserializer::from_slice(body);
    (&mut acc).deserialize(&mut de)?;
    de.end()?;

    let Acc { mut out, order } = acc;
    let (n, dim) = (out.len(), out.dim);
    // Providers return input order in practice; reorder by `index` if one does not.
    if order.len() == n && order.iter().enumerate().any(|(i, &o)| i != o) {
        let mut src = vec![usize::MAX; n];
        for (i, &o) in order.iter().enumerate() {
            if let Some(s) = src.get_mut(o) {
                *s = i;
            }
        }
        if src.iter().all(|&s| s != usize::MAX) {
            out.data = src.iter().flat_map(|&i| &out.data[i * dim..(i + 1) * dim]).copied().collect();
        }
    }
    Ok(out)
}

/// Streams the response straight into one flat `Vec<f32>`: no per-vector allocations.
#[derive(Default)]
struct Acc {
    out: Embeddings,
    order: Vec<usize>,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "lowercase")]
enum Key {
    Data,
    Model,
    Usage,
    Embedding,
    Index,
    #[serde(other)]
    Other,
}

impl<'de> DeserializeSeed<'de> for &mut Acc {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for &mut Acc {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an embeddings response")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<(), A::Error> {
        while let Some(k) = m.next_key::<Key>()? {
            match k {
                Key::Data => m.next_value_seed(Items(&mut *self))?,
                Key::Model => self.out.model = m.next_value::<Option<String>>()?.unwrap_or_default(),
                Key::Usage => {
                    if let Some(u) = m.next_value::<Option<OaUsage>>()? {
                        self.out.usage = u.into();
                    }
                }
                _ => {
                    m.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

struct Items<'a>(&'a mut Acc);

impl<'de> DeserializeSeed<'de> for Items<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Items<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list of embeddings")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(Item(&mut *self.0))?.is_some() {}
        Ok(())
    }
}

struct Item<'a>(&'a mut Acc);

impl<'de> DeserializeSeed<'de> for Item<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Item<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an embedding object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<(), A::Error> {
        let acc = self.0;
        while let Some(k) = m.next_key::<Key>()? {
            match k {
                Key::Embedding => {
                    let n = m.next_value_seed(Floats(&mut acc.out.data))?;
                    if acc.out.dim == 0 {
                        acc.out.dim = n;
                    } else if n != acc.out.dim {
                        return Err(de::Error::custom("embeddings have different lengths"));
                    }
                }
                Key::Index => acc.order.push(m.next_value()?),
                _ => {
                    m.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

/// Appends a JSON number array to the buffer, returns how many it added.
struct Floats<'a>(&'a mut Vec<f32>);

impl<'de> DeserializeSeed<'de> for Floats<'_> {
    type Value = usize;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<usize, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Floats<'_> {
    type Value = usize;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array of floats")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<usize, A::Error> {
        let start = self.0.len();
        while let Some(x) = seq.next_element::<f32>()? {
            self.0.push(x);
        }
        Ok(self.0.len() - start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_and_reordered() {
        let e = parse(
            br#"{"object":"list","data":[
            {"object":"embedding","index":1,"embedding":[3,4.5]},
            {"object":"embedding","index":0,"embedding":[1,2]}],
            "model":"e","usage":{"prompt_tokens":4,"total_tokens":4}}"#,
        )
        .unwrap();
        assert_eq!((e.len(), e.dim()), (2, 2));
        assert_eq!(e.get(0), Some(&[1.0, 2.0][..]));
        assert_eq!(e.iter().nth(1), Some(&[3.0, 4.5][..]));
        assert_eq!(e.usage.input_tokens, 4);
        assert!(parse(br#"{"data":[{"embedding":[1]},{"embedding":[1,2]}]}"#).is_err());
    }
}
