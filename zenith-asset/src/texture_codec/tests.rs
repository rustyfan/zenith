use super::*;
use crate::import::MAX_ARTIFACT_BYTES;

fn assert_texture_eq(actual: &Texture, expected: &Texture) {
    assert_eq!(actual.width, expected.width);
    assert_eq!(actual.height, expected.height);
    assert_eq!(actual.format, expected.format);
    assert_eq!(actual.pixels, expected.pixels);
    assert_eq!(actual.is_cubemap, expected.is_cubemap);
    assert_eq!(actual.mip_levels, expected.mip_levels);
}

fn round_trip(texture: &Texture) {
    let legacy = <SerdeCodec as AssetCodec<Texture>>::encode(&SerdeCodec, texture).unwrap();
    let bulk = TextureCodec.encode(texture).unwrap();
    assert_eq!(bulk, legacy);
    assert_texture_eq(&TextureCodec.decode(&legacy).unwrap(), texture);
    let decoded = <SerdeCodec as AssetCodec<Texture>>::decode(&SerdeCodec, &bulk).unwrap();
    assert_texture_eq(&decoded, texture);
}

#[test]
fn existing_v2_texture_payload_is_unchanged() {
    let fixture = [2, 1, 2, 8, 0, 1, 2, 3, 252, 253, 254, 255, 0, 1];
    let texture = Texture {
        width: 2,
        height: 1,
        format: TextureFormat::Rgba8Unorm,
        pixels: vec![0, 1, 2, 3, 252, 253, 254, 255],
        is_cubemap: false,
        mip_levels: 1,
    };
    assert_texture_eq(&TextureCodec.decode(&fixture).unwrap(), &texture);
    assert_eq!(TextureCodec.encode(&texture).unwrap(), fixture);
    assert_eq!(
        TextureCodec.key(),
        <SerdeCodec as AssetCodec<Texture>>::key(&SerdeCodec)
    );
    round_trip(&texture);
}

#[test]
fn all_formats_mips_layers_and_length_prefixes_remain_compatible() {
    use TextureFormat::*;
    for format in [
        R8Unorm,
        Rg8Unorm,
        Rgba8Unorm,
        Rgba8Srgb,
        R16Unorm,
        Rg16Unorm,
        Rgba16Unorm,
        Rgba16Float,
        Rgba32Float,
        Bc5Unorm,
        Bc7Unorm,
        Bc7Srgb,
        Bc6hUfloat,
        Bc6hSfloat,
    ] {
        for is_cubemap in [false, true] {
            for mip_levels in [1, 3] {
                let length = (0..mip_levels)
                    .map(|mip| format.data_size_in_bytes(4 >> mip, 4 >> mip))
                    .sum::<usize>()
                    * if is_cubemap { 6 } else { 1 };
                let texture = Texture {
                    width: 4,
                    height: 4,
                    format,
                    pixels: (0..length).map(|i| i as u8).collect(),
                    is_cubemap,
                    mip_levels,
                };
                texture.validate().unwrap();
                round_trip(&texture);
            }
        }
    }
    for length in [0, 1, 250, 251, 252, 65535, 65536] {
        round_trip(&Texture {
            width: 1,
            height: 1,
            format: R8Unorm,
            pixels: (0..length).map(|i| i as u8).collect(),
            is_cubemap: false,
            mip_levels: 1,
        });
    }
}

#[test]
fn malformed_and_oversized_pixel_buffers_are_rejected() {
    let fixture = [2, 1, 2, 8, 0, 1, 2, 3, 252, 253, 254, 255, 0, 1];
    for end in 0..fixture.len() {
        assert!(TextureCodec.decode(&fixture[..end]).is_err());
    }
    let mut trailing = fixture.to_vec();
    trailing.push(0);
    assert!(TextureCodec.decode(&trailing).is_err());
    for length in [MAX_ARTIFACT_BYTES as u64 + 1, u64::MAX] {
        let mut payload = vec![1, 1, 0];
        payload.extend(bincode::serde::encode_to_vec(length, bincode::config::standard()).unwrap());
        assert!(TextureCodec.decode(&payload).is_err());
    }
    let mut invalid_format = fixture;
    invalid_format[2] = 250;
    assert!(TextureCodec.decode(&invalid_format).is_err());
    let mut invalid_bool = fixture;
    invalid_bool[12] = 2;
    assert!(TextureCodec.decode(&invalid_bool).is_err());
}
