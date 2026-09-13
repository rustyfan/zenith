use super::{Temp, *};
use crate::{
    gltf::{GltfImporter, GltfSettings},
    hdr::{HdrImporter, HdrSettings},
    mesh::{Mesh, Scene},
    texture::{
        Texture, TextureCompression, TextureFormat, TextureSettings, TextureUsage, bake_image,
    },
};
use glam::{Mat4, Vec3};
use std::io::Cursor;

fn png(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    let image = image::DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(
        width,
        height,
        image::Rgba(pixel),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
fn fixture() -> Arc<MemorySource> {
    let source = Arc::new(MemorySource::default());
    let positions = [
        [0.0f32, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
    ];
    let mut buffer = bytemuck::cast_slice(&positions).to_vec();
    buffer.extend_from_slice(bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]));
    source.insert("model/data.bin", buffer).unwrap();
    source
        .insert("model/image.png", png(3, 5, [128, 128, 255, 255]))
        .unwrap();
    let document = serde_json::json!({
        "asset":{"version":"2.0"}, "scene":1,
        "scenes":[{"nodes":[0]},{"nodes":[0,1]}],
        "nodes":[{"mesh":0,"translation":[1,2,3]},{"mesh":0,"translation":[4,5,6]}],
        "buffers":[{"uri":"data.bin","byteLength":72}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":48},{"buffer":0,"byteOffset":48,"byteLength":24}],
        "accessors":[
            {"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[0,0,0],"max":[1,1,1]},
            {"bufferView":1,"componentType":5125,"count":6,"type":"SCALAR"},
            {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}
        ],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1},{"attributes":{"POSITION":0},"indices":1,"material":0}]},
            {"primitives":[{"attributes":{"POSITION":2}}]}],
        "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}},"normalTexture":{"index":0}}],
        "textures":[{"source":0}], "images":[{"uri":"image.png"}]
    });
    source
        .insert("model/scene.gltf", serde_json::to_vec(&document).unwrap())
        .unwrap();
    source
}
#[test]
fn gltf_preserves_scenes_instances_defaults_and_usage_variants() {
    let cache = Temp::new();
    let source = fixture();
    let server = AssetServer::builder()
        .source(source.clone())
        .cache_dir(&cache.0)
        .with_builtin_assets()
        .build()
        .unwrap();
    let handle = server
        .load_with::<GltfImporter>(
            "model/scene.gltf",
            &GltfSettings {
                y_up_to_z_up: false,
                ..Default::default()
            },
        )
        .unwrap();
    let scene = handle.wait().unwrap();
    assert_eq!(scene.nodes.len(), 2);
    assert_eq!(scene.instances.len(), 4);
    assert_eq!(scene.instances[0].mesh.id(), scene.instances[2].mesh.id());
    assert_eq!(
        Mat4::from_cols_array(&scene.instances[0].transform).transform_point3(Vec3::ZERO),
        Vec3::new(1.0, 2.0, 3.0)
    );
    assert_eq!(
        Mat4::from_cols_array(&scene.instances[2].transform).transform_point3(Vec3::ZERO),
        Vec3::new(4.0, 5.0, 6.0)
    );
    let mesh = scene.instances[0].mesh.get().unwrap();
    assert_eq!(mesh.vertices.len(), 6);
    assert_eq!(mesh.indices, vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(mesh.vertices[0].normal, [0.0, 0.0, 1.0]);
    assert_ne!(mesh.vertices[3].normal, mesh.vertices[0].normal);
    let default = scene.instances[0].material.get().unwrap();
    assert_eq!(default.base_color, [1.0; 4]);
    assert_eq!((default.metallic, default.roughness), (1.0, 1.0));
    let material = scene.instances[1].material.get().unwrap();
    let color = material.base_color_tex.as_ref().unwrap();
    let normal = material.normal_tex.as_ref().unwrap();
    assert_ne!(color.id(), normal.id());
    assert_eq!(color.get().unwrap().format, TextureFormat::Bc7Srgb);
    assert_eq!(normal.get().unwrap().format, TextureFormat::Bc5Unorm);
    assert_eq!(color.get().unwrap().pixels.len(), 64);
    let alternate =
        AssetPath::<Scene>::from_address(handle.address().unwrap().with_label("scenes/0").unwrap());
    assert_eq!(
        server
            .load_path(&alternate)
            .unwrap()
            .wait()
            .unwrap()
            .instances
            .len(),
        2
    );
    let nonindexed = AssetPath::<Mesh>::from_address(
        handle
            .address()
            .unwrap()
            .with_label("meshes/1/primitives/0")
            .unwrap(),
    );
    assert_eq!(
        server
            .load_path(&nonindexed)
            .unwrap()
            .wait()
            .unwrap()
            .indices,
        vec![0, 1, 2]
    );
    let before = handle.snapshot().unwrap().revision;
    source
        .insert("model/image.png", png(3, 5, [200, 128, 255, 255]))
        .unwrap();
    server.reload(&handle).unwrap();
    handle.wait().unwrap();
    assert!(handle.snapshot().unwrap().revision > before);
    assert_eq!(server.stats().imports, 2);
    let path = AssetPath::<Scene>::from_address(handle.address().unwrap().clone());
    drop(server);
    let runtime = AssetServer::builder()
        .cache_dir(&cache.0)
        .with_builtin_assets()
        .packaged()
        .build()
        .unwrap();
    let cached = runtime.load_path(&path).unwrap().wait().unwrap();
    assert_eq!(cached.instances.len(), 4);
    assert_eq!(runtime.stats().imports, 0);
    assert_eq!(cached.instances[0].mesh.id(), cached.instances[2].mesh.id());
}

#[test]
fn gltf_imports_authored_tangents_and_generates_missing_tangents() {
    for authored in [false, true] {
        let source = MemorySource::default();
        let mut bytes = Vec::new();
        for values in [
            vec![0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0].repeat(3),
            vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, -1.0].repeat(3),
        ] {
            bytes.extend_from_slice(bytemuck::cast_slice(&values));
        }
        source.insert("data.bin", bytes).unwrap();
        let mut document = serde_json::json!({
            "asset":{"version":"2.0"}, "scene":0,
            "scenes":[{"nodes":[0]}], "nodes":[{"mesh":0}],
            "buffers":[{"uri":"data.bin","byteLength":144}],
            "bufferViews":[
                {"buffer":0,"byteOffset":0,"byteLength":36},
                {"buffer":0,"byteOffset":36,"byteLength":36},
                {"buffer":0,"byteOffset":72,"byteLength":24},
                {"buffer":0,"byteOffset":96,"byteLength":48}
            ],
            "accessors":[
                {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},
                {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"},
                {"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"},
                {"bufferView":3,"componentType":5126,"count":3,"type":"VEC4"}
            ],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}}]}]
        });
        if authored {
            document["meshes"][0]["primitives"][0]["attributes"]["TANGENT"] = 3.into();
        }
        source
            .insert("scene.gltf", serde_json::to_vec(&document).unwrap())
            .unwrap();
        let cache = Temp::new();
        let server = AssetServer::builder()
            .source(source)
            .cache_dir(&cache.0)
            .with_builtin_assets()
            .build()
            .unwrap();
        let handle = server.load_blocking::<Scene>("scene.gltf").unwrap();
        let expected = if authored {
            [0.0, 1.0, 0.0, -1.0]
        } else {
            [1.0, 0.0, 0.0, 1.0]
        };
        let mesh = handle.get().unwrap().instances[0].mesh.get().unwrap();
        assert!(mesh.vertices.iter().all(|v| v.tangent == expected));
        assert_eq!(mesh.vertices_bytes().len(), 3 * 48);
        let path = AssetPath::<Scene>::from_address(handle.address().unwrap().clone());
        drop(server);
        let runtime = AssetServer::builder()
            .cache_dir(&cache.0)
            .with_builtin_assets()
            .packaged()
            .build()
            .unwrap();
        let cached = runtime.load_path(&path).unwrap().wait().unwrap();
        assert!(
            cached.instances[0]
                .mesh
                .get()
                .unwrap()
                .vertices
                .iter()
                .all(|v| v.tangent == expected)
        );
        assert_eq!(runtime.stats().imports, 0);
    }
}

#[test]
fn texture_mips_preserve_odd_edges_and_numeric_semantics() {
    let odd = image::DynamicImage::ImageRgba8(image::ImageBuffer::from_fn(3, 1, |x, _| {
        image::Rgba([if x == 2 { 255 } else { 0 }, 0, 0, 255])
    }));
    let linear = bake_image(
        &odd,
        &TextureSettings {
            usage: TextureUsage::Linear,
            compression: TextureCompression::None,
            mipmaps: true,
        },
    )
    .unwrap();
    assert_eq!(linear.pixels[12], 85);
    let srgb = bake_image(
        &odd,
        &TextureSettings {
            compression: TextureCompression::None,
            ..Default::default()
        },
    )
    .unwrap();
    assert!((155..=157).contains(&srgb.pixels[12]));
    let precise = image::DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        1,
        1,
        image::Rgba([32768, 16384, 65535, 65535]),
    ));
    let linear = bake_image(
        &precise,
        &TextureSettings {
            usage: TextureUsage::Linear,
            compression: TextureCompression::None,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(linear.format, TextureFormat::Rgba16Unorm);
    assert_eq!(
        u16::from_le_bytes(linear.pixels[..2].try_into().unwrap()),
        32768
    );
    for (w, h) in [(1, 1), (2, 2), (3, 5), (5, 3)] {
        let image = image::DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(
            w,
            h,
            image::Rgba([128, 128, 255, 255]),
        ));
        for usage in [
            TextureUsage::Color,
            TextureUsage::Normal,
            TextureUsage::Linear,
        ] {
            let texture = bake_image(
                &image,
                &TextureSettings {
                    usage,
                    ..Default::default()
                },
            )
            .unwrap();
            texture.validate().unwrap();
            assert_eq!(texture.mip_levels, w.max(h).ilog2() + 1);
        }
    }
}
#[test]
fn hdr_handles_tiny_bc6h_tails_and_uncompressed_float_output() {
    let source = Arc::new(MemorySource::default());
    let mut bytes = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut bytes)
        .encode(&vec![image::Rgb([2.0, 1.0, 0.5]); 32], 8, 4)
        .unwrap();
    source.insert("sky.hdr", bytes).unwrap();
    let server = AssetServer::builder()
        .source(source)
        .with_builtin_assets()
        .build()
        .unwrap();
    for compression in [TextureCompression::Block, TextureCompression::None] {
        let texture = server
            .load_with::<HdrImporter>(
                "sky.hdr",
                &HdrSettings {
                    face_size: Some(2),
                    mipmaps: true,
                    compression,
                },
            )
            .unwrap()
            .wait()
            .unwrap();
        assert!(texture.is_cubemap);
        assert_eq!(texture.mip_levels, 2);
        texture.validate().unwrap();
        if compression == TextureCompression::None {
            assert_eq!(texture.format, TextureFormat::Rgba16Float);
            assert_eq!(
                half::f16::from_bits(u16::from_le_bytes(texture.pixels[..2].try_into().unwrap()))
                    .to_f32(),
                2.0
            );
        } else {
            assert_eq!(texture.format, TextureFormat::Bc6hUfloat);
            assert_eq!(texture.pixels.len(), 192);
        }
    }
}
#[test]
#[ignore = "imports the included production scene and 4K HDR environment"]
fn included_assets_bake_and_load_from_relocated_package() {
    let cache = Temp::new();
    let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../content");
    let source = FileSource::new(content);
    let server = AssetServer::builder()
        .source(source)
        .cache_dir(&cache.0)
        .with_builtin_assets()
        .build()
        .unwrap();
    let started = Instant::now();
    let scene = server
        .load_blocking::<Scene>("mesh/cerberus/scene.gltf")
        .unwrap();
    let sky = server
        .load_blocking::<Texture>("texture/minedump_flats_4k.hdr")
        .unwrap();
    assert!(!scene.get().unwrap().instances.is_empty());
    sky.get().unwrap().validate().unwrap();
    eprintln!("cold bake: {:?}; {:?}", started.elapsed(), server.stats());
    drop(server);
    let moved = Temp::new();
    for directory in ["manifests", "blobs"] {
        fs::rename(cache.0.join(directory), moved.0.join(directory)).unwrap();
    }
    let runtime = AssetServer::builder()
        .cache_dir(&moved.0)
        .packaged()
        .with_builtin_assets()
        .build()
        .unwrap();
    let started = Instant::now();
    let cached = runtime
        .load_blocking::<Scene>("mesh/cerberus/scene.gltf")
        .unwrap();
    let environment = runtime
        .load_blocking::<Texture>("texture/minedump_flats_4k.hdr")
        .unwrap();
    assert_eq!(
        cached.get().unwrap().instances.len(),
        scene.get().unwrap().instances.len()
    );
    assert_eq!(environment.get().unwrap().pixels, sky.get().unwrap().pixels);
    assert_eq!(runtime.stats().imports, 0);
    eprintln!(
        "packaged load: {:?}; {:?}",
        started.elapsed(),
        runtime.stats()
    );
}

#[test]
fn tga_uses_the_registered_extension_and_float_compression_rejects_clamping() {
    let source = MemorySource::default();
    let image = image::DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(
        2,
        2,
        image::Rgba([255, 0, 0, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Tga).unwrap();
    source.insert("red.tga", bytes.into_inner()).unwrap();
    let server = AssetServer::builder()
        .source(source)
        .with_builtin_assets()
        .build()
        .unwrap();
    let texture = server.load_blocking::<Texture>("red.tga").unwrap();
    texture.get().unwrap().validate().unwrap();
    let floating = image::DynamicImage::ImageRgba32F(image::ImageBuffer::from_pixel(
        1,
        1,
        image::Rgba([2.0, 1.0, 0.0, 1.0]),
    ));
    assert!(bake_image(&floating, &TextureSettings::default()).is_err());
    let texture = bake_image(
        &floating,
        &TextureSettings {
            compression: TextureCompression::None,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        f32::from_le_bytes(texture.pixels[..4].try_into().unwrap()),
        2.0
    );
}
