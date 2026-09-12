use crate::{
    AssetCodec, Result, SerdeCodec,
    texture::{Texture, TextureFormat},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error, Visitor},
};
use std::{borrow::Cow, fmt};

pub(crate) struct TextureCodec;

#[derive(Serialize, Deserialize)]
struct TextureData<'a> {
    width: u32,
    height: u32,
    format: TextureFormat,
    #[serde(
        serialize_with = "serialize_pixels",
        deserialize_with = "deserialize_pixels"
    )]
    pixels: Cow<'a, [u8]>,
    is_cubemap: bool,
    mip_levels: u32,
}

impl AssetCodec<Texture> for TextureCodec {
    fn key(&self) -> &'static str {
        <SerdeCodec as AssetCodec<Texture>>::key(&SerdeCodec)
    }
    fn encode(&self, data: &Texture) -> Result<Vec<u8>> {
        SerdeCodec::encode_data(&TextureData {
            width: data.width,
            height: data.height,
            format: data.format,
            pixels: Cow::Borrowed(&data.pixels),
            is_cubemap: data.is_cubemap,
            mip_levels: data.mip_levels,
        })
    }
    fn decode(&self, bytes: &[u8]) -> Result<Texture> {
        let data: TextureData<'_> = SerdeCodec::decode_data(bytes)?;
        Ok(Texture {
            width: data.width,
            height: data.height,
            format: data.format,
            pixels: data.pixels.into_owned(),
            is_cubemap: data.is_cubemap,
            mip_levels: data.mip_levels,
        })
    }
}

fn serialize_pixels<S: Serializer>(
    pixels: &[u8],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_bytes(pixels)
}

fn deserialize_pixels<'de, 'a, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Cow<'a, [u8]>, D::Error> {
    struct Pixels;
    impl<'de> Visitor<'de> for Pixels {
        type Value = Vec<u8>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("texture pixel bytes")
        }
        fn visit_bytes<E: Error>(self, bytes: &[u8]) -> std::result::Result<Self::Value, E> {
            Ok(bytes.to_vec())
        }
        fn visit_byte_buf<E: Error>(self, bytes: Vec<u8>) -> std::result::Result<Self::Value, E> {
            Ok(bytes)
        }
    }
    deserializer.deserialize_byte_buf(Pixels).map(Cow::Owned)
}

#[cfg(test)]
mod tests;
