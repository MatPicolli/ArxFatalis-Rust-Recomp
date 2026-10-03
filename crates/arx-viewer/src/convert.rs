//! Conversion of Arx assets (images, `.ftl` objects) into Bevy assets.

use arx_formats::{PakSet, ftl::{Face, Ftl}};
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    mesh::PrimitiveTopology,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use crate::animated::{MeshSrc, no_cull};
use std::{collections::HashMap, sync::Arc};

use crate::Shown;

/// A decoded texture. `keyed` is set when it has transparent pixels (colour-keyed `.bmp`s, alpha TGAs).
#[derive(Clone)]
pub struct TexInfo {
    pub handle: Handle<Image>,
    pub keyed: bool,
}

#[derive(Resource, Default)]
pub struct TextureCache(HashMap<String, Option<TexInfo>>);

/// Convert an Arx coordinate (+Y down, +Z forward) into Bevy's (+Y up, -Z forward). This is a
/// 180 degree rotation about X, so handedness and triangle winding are preserved.
pub fn to_bevy(p: [f32; 3]) -> [f32; 3] {
    [p[0], -p[1], -p[2]]
}

/// Decode an image from the archives. `.bmp` textures use black as a colour key (transparent),
/// like the original engine. Returns the image and whether it contains transparent pixels.
fn decode(pak: &PakSet, path: &str) -> Option<(Image, bool)> {
    let bytes = pak.read(path).ok()?;
    let format = match path.rsplit('.').next()? {
        "bmp" => image::ImageFormat::Bmp,
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        "tga" => image::ImageFormat::Tga,
        "png" => image::ImageFormat::Png,
        _ => return None,
    };
    let img = image::load_from_memory_with_format(&bytes, format).ok()?;
    let (w, h) = (img.width(), img.height());
    let mut rgba = img.into_rgba8().into_raw();
    let mut keyed = false;
    if format == image::ImageFormat::Bmp {
        for px in rgba.chunks_exact_mut(4) {
            if px[0] == 0 && px[1] == 0 && px[2] == 0 {
                px[3] = 0;
                keyed = true;
            }
        }
    }
    keyed |= rgba.chunks_exact(4).any(|px| px[3] < 255);

    // Full mip chain, stored level after level.
    let base = image::RgbaImage::from_raw(w, h, rgba).expect("rgba buffer matches dimensions");
    let mut data = base.as_raw().clone();
    let mut mips = 1;
    let mut level = base;
    while level.width() > 1 || level.height() > 1 {
        let (nw, nh) = ((level.width() / 2).max(1), (level.height() / 2).max(1));
        level = image::imageops::resize(&level, nw, nh, image::imageops::FilterType::Triangle);
        data.extend_from_slice(level.as_raw());
        mips += 1;
    }
    let mut image = Image::new_uninit(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.mip_level_count = mips;
    image.data = Some(data);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..ImageSamplerDescriptor::linear()
    });
    Some((image, keyed))
}

pub fn load_texture(
    pak: &PakSet,
    name: &str,
    cache: &mut TextureCache,
    images: &mut Assets<Image>,
) -> Option<TexInfo> {
    let resolved = pak.find_texture(name).or_else(|| pak.contains(name).then(|| name.to_owned()))?;
    cache
        .0
        .entry(resolved.clone())
        .or_insert_with(|| decode(pak, &resolved).map(|(img, keyed)| TexInfo { handle: images.add(img), keyed }))
        .clone()
}

pub struct ModelSpawn {
    pub bounds: (Vec3, Vec3),
    pub ftl: Arc<Ftl>,
    pub meshes: Vec<MeshSrc>,
}

pub fn spawn_model(
    commands: &mut Commands,
    pak: &PakSet,
    path: &str,
    cache: &mut TextureCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Option<ModelSpawn> {
    let ftl = match pak.read(path).map_err(|e| e.to_string()).and_then(|b| Ftl::parse(&b).map_err(|e| e.to_string())) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{path}: {e}");
            return None;
        }
    };

    // Group faces by (material, double sided) so each group is a single draw call.
    let mut groups: HashMap<(Option<u16>, bool), Vec<&Face>> = HashMap::new();
    for f in &ftl.faces {
        // Faces of type 0 are flat shaded and never textured.
        let material = if f.facetype == 0 { None } else { f.material };
        groups.entry((material, f.facetype & Face::DOUBLE_SIDED != 0)).or_default().push(f);
    }

    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in &ftl.vertices {
        let p = Vec3::from(to_bevy(v.pos));
        min = min.min(p);
        max = max.max(p);
    }

    let mut spawned = Vec::new();
    for ((material, double_sided), faces) in groups {
        let mut positions = Vec::with_capacity(faces.len() * 3);
        let mut uvs = Vec::with_capacity(faces.len() * 3);
        let mut src = Vec::with_capacity(faces.len() * 3);
        for f in faces {
            for k in 0..3 {
                positions.push(to_bevy(ftl.vertices[f.vid[k] as usize].pos));
                uvs.push([f.u[k], f.v[k]]);
                src.push(f.vid[k] as u32);
            }
        }
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.compute_flat_normals();

        let texture = material
            .and_then(|m| ftl.textures.get(m as usize))
            .filter(|t| !t.is_empty())
            .and_then(|t| load_texture(pak, t, cache, images));
        let keyed = texture.as_ref().is_some_and(|t| t.keyed);
        let mat = StandardMaterial {
            base_color_texture: texture.map(|t| t.handle),
            base_color: if material.is_none() { Color::srgb(0.6, 0.6, 0.6) } else { Color::WHITE },
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            alpha_mode: if keyed { AlphaMode::Mask(0.5) } else { AlphaMode::Opaque },
            cull_mode: if double_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
            double_sided,
            ..default()
        };
        let handle = meshes.add(mesh);
        spawned.push(MeshSrc { handle: handle.clone(), src });
        commands.spawn((Shown, Mesh3d(handle), MeshMaterial3d(materials.add(mat)), no_cull()));
    }
    Some(ModelSpawn { bounds: (min, max), ftl: Arc::new(ftl), meshes: spawned })
}

pub fn spawn_texture(
    commands: &mut Commands,
    pak: &PakSet,
    path: &str,
    cache: &mut TextureCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Option<(Vec3, Vec3)> {
    let (image, keyed) = decode(pak, path)?;
    let (w, h) = (image.width() as f32, image.height() as f32);
    let handle = images.add(image);
    cache.0.insert(path.to_owned(), Some(TexInfo { handle: handle.clone(), keyed }));
    let mat = StandardMaterial {
        base_color_texture: Some(handle),
        unlit: true,
        alpha_mode: AlphaMode::Mask(0.5),
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    commands.spawn((
        Shown,
        Mesh3d(meshes.add(Rectangle::new(w, h))),
        MeshMaterial3d(materials.add(mat)),
    ));
    Some((Vec3::new(-w / 2.0, -h / 2.0, 0.0), Vec3::new(w / 2.0, h / 2.0, 0.0)))
}
