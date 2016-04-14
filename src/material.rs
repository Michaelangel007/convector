//! Determines how light bounces off a surface.
//!
//! # Material encoding
//!
//! A material is associated with every surface. It is a 32-bit value that
//! consists of the following parts:
//!
//!  * Bit 31 (sign bit): if 1, the material is emissive, if 0,
//!    the material is not.
//!
//!  * Bit 30: if 1, a primitive with this material is eligible for direct
//!    sampling.
//!
//!  * Bit 29: if 1, this material is a glass material.
//!
//!  * Bits 24-37 contain the texture index ranging from 0 to 7.
//!
//!  * Bits 0-23 contain the RGB color of the material, red in the least
//!    significant bits, blue in the most significant bits.
//!
//! # A note on CPU and GPU shading
//!
//! Doing texture lookup and filtering on the CPU is extremely expensive.
//! Looking up four pixels in a bitmap is essentially doing random access into
//! memory. Everything will be a cache miss, and it will trash the cache for
//! other data too. The GPU is optimized for this kind of thing, so it would be
//! nice if we could to texture lookup and filtering there.
//!
//! Without fancy materials, every bounce off a surface multiplies the pixel
//! color by a factor for each channel. To compute the final color of a pixel,
//! we collect a color at every bounce, and multiply all of these together. To
//! avoid texture lookup on the CPU, we could send a buffer with texture
//! coordinates to the GPU per bounce, and do the lookup there. However, that is
//! a lot of data even for a few bounces. Sending a 720p R8G8B8A8 buffer to the
//! GPU takes about 2 ms already, and we don't want to spend the entire frame
//! doing texture upload. So here's an idea:
//!
//!  * If textures are only used to add a bit of detail, not for surfaces that
//!    vary wildly in color, then after one bounce we could simply not sample
//!    the texture, and take an average surface color. For diffuse surfaces
//!    light from all directions is mixed anyway, so the error is very small.
//!
//!  * We can store the average surface color with the material and compute
//!    all the shading on the CPU, except for the contribution of the first
//!    bounce. Then send only one set of texture coordinates to the GPU, plus
//!    the color computed on the CPU.
//!
//!  * For pure specular reflections, the texture lookup can be postponed to the
//!    next bounce. It does not matter for which bounce we do the lookup, but we
//!    can only do one per pixel.

use random::Rng;
use ray::{MIntersection, MRay};
use scene::Scene;
use simd::Mf32;
use std::f32::consts;
use vector3::MVector3;

#[derive(Copy, Clone, Debug)]
pub struct SMaterial(u32);

pub type MMaterial = Mf32;

pub struct MDirectSample {
    pub position: MVector3,
    pub normal: MVector3,
    pub area: Mf32,
}

impl SMaterial {
    pub fn sky() -> SMaterial {
        // Set only the emissive bit.
        // TODO: Disable the direct sampling bit once I have glass windows.
        let mat = 0b11000000_00000000_00000000_00000000_u32;
        SMaterial(mat)
    }

    /// A white diffuse material.
    pub fn white() -> SMaterial {
        SMaterial::diffuse(255, 255, 255)
    }

    /// A diffuse material with the given color.
    pub fn diffuse(r: u8, g: u8, b: u8) -> SMaterial {
        let mat = ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
        SMaterial(mat)
    }

    /// A transparent and reflective material.
    pub fn glass() -> SMaterial {
        let mat = 0b0110_0000_00000000_00000000_00000000_u32;
        SMaterial(mat)
    }

    /// Returns whether the material is eligible for direct sampling.
    pub fn is_direct_sample(&self) -> bool {
        let ds_mask = 0b01000000_00000000_00000000_00000000;
        let SMaterial(mat) = *self;
        (mat & ds_mask) == ds_mask
    }
}

impl MMaterial {
    pub fn broadcast_material(material: SMaterial) -> MMaterial {
        use std::mem::transmute;
        let SMaterial(mat) = material;
        let matf: f32 = unsafe { transmute(mat) };
        Mf32::broadcast(matf)
    }

    pub fn sky() -> MMaterial {
        MMaterial::broadcast_material(SMaterial::sky())
    }
}

/// Returns the sky color for a ray in the given direction.
pub fn sky_intensity(ray_direction: MVector3) -> MVector3 {
    // TODO: Better sky model.
    let up = MVector3::new(Mf32::zero(), Mf32::zero(), Mf32::one());
    let half = Mf32::broadcast(0.5);
    let d = ray_direction.dot(up).mul_add(half, half);
    let r = d;
    let g = d * d;
    let b = d * (d * d);
    MVector3::new(r, g, b).mul_add(half, MVector3::new(half, half, half))
}

/// Continues the path of a photon by sampling the BRDF.
///
/// Returns the new ray, the probability density for that ray, and the color
/// modulation for the bounce.
#[inline(always)]
fn continue_path_brdf(ray: &MRay,
                      isect: &MIntersection,
                      rng: &mut Rng)
                      -> (MRay, Mf32, MVector3) {
    // Bounce in a random direction in the hemisphere around the surface
    // normal, with a cosine-weighted distribution, for a diffuse bounce.
    let dir_z = rng.sample_hemisphere_vector();
    let direction = dir_z.rotate_hemisphere(isect.normal);

    // Build a new ray, offset by an epsilon from the intersection so we
    // don't intersect the same surface again.
    let origin = direction.mul_add(Mf32::epsilon(), isect.position);
    let new_ray = MRay {
        origin: origin,
        direction: direction,
        active: Mf32::zero(),
    };

    // The probability density for the ray is dot(normal, direction) divided by
    // the intgral of that over the hemisphere (which happens to be pi).
    let pd = dir_z.z * Mf32::broadcast(1.0 / consts::PI);

    // There is the factor dot(normal, direction) that modulates the
    // incoming contribution. The incoming energy is then radiated evenly in all
    // directions (the diffuse assumption), so the integral over the hemisphere
    // of that factor (excluding the dot, that one was for _incoming_ energy)
    // should be 1. The area of the hemisphere is 2pi, so divide by that.
    let modulation = Mf32::broadcast(0.5 / consts::PI) * dir_z.z;
    let color_mod = MVector3::new(modulation, modulation, modulation);

    (new_ray, pd, color_mod)
}

fn continue_path_direct_sample(scene: &Scene,
                               isect: &MIntersection,
                               rng: &mut Rng)
                               -> (MVector3, MRay) {
    let (ds, num) = scene.get_direct_sample(rng);

    // TODO: Get multiple samples and do resampled importance sampling.

    let to_surf = ds.position - isect.position;
    let distance_sqr = to_surf.norm_squared();
    let direction = to_surf * distance_sqr.rsqrt();

    let dot_emissive = -ds.normal.dot(direction); // TODO: or abs? Do I ever sample back sides?
    let dot_surface = isect.normal.dot(direction);

    // Build a new ray, offset by an epsilon from the intersection so we
    // don't intersect the same surface again.
    let origin = direction.mul_add(Mf32::epsilon(), isect.position);
    let new_ray = MRay {
        origin: origin,
        direction: direction,
        active: Mf32::zero(),
    };

    // TODO: What if two direct sampling surfaces overlap? Then the result is
    // not correct any more, there needs to be a true visibility ray. Except
    // when using MIS?

    let cosines = dot_emissive * dot_surface;
    let direct_factor = Mf32::broadcast(num as f32) * ds.area;
    let norm_factor = (direct_factor * cosines) * distance_sqr.recip_fast();
    let color = MVector3::new(norm_factor, norm_factor, norm_factor);

    (color, new_ray)
}

/// Continues the path of a photon.
///
/// If a ray intersected a surface with a certain material, then this will
/// compute the ray that continues the light path. A factor to multiply the
/// final color by is returned as well.
pub fn continue_path(scene: &Scene,
                     ray: &MRay,
                     isect: &MIntersection,
                     rng: &mut Rng)
                     -> (MRay, MVector3) {
    // Emissive materials have the sign bit set to 1, and a sign bit of 1
    // means that the ray is inactive. So hitting an emissive material
    // deactivates the ray: there is no need for an additional bounce.
    let active = ray.active | isect.material;

    let (brdf_ray, brdf_pd, brdf_mod) = continue_path_brdf(ray, isect, rng);
    let color_mod = brdf_mod * brdf_pd.recip_fast();
    let new_ray = MRay {
        origin: brdf_ray.origin.pick(ray.origin, active),
        direction: brdf_ray.direction.pick(ray.direction, active),
        active: active,
    };

    // let (direct_pd, direct_ray) = continue_path_direct_sample(scene, isect, rng);
    // let color_mod = direct_mod * direct_pd.recip();
    // let new_ray = MRay {
    //     origin: direct_ray.origin.pick(ray.origin, active),
    //     direction: direct_ray.direction.pick(ray.direction, active),
    //     active: active,
    // };

    let white = MVector3::new(Mf32::one(), Mf32::one(), Mf32::one());
    let color_mod = color_mod.pick(white, active);

    (new_ray, color_mod)
}
