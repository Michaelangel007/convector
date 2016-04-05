//! Determines how light bounces off a surface.
//!
//! # Material index encoding
//!
//! A 32-bit material index is associated with every surface. This value encodes
//! some material infomation. Additional data must be looked up in the material
//! bank. The material index consists of the following parts:
//!
//!  * Bit 31 (sign bit): if 1, the material is emissive, if 0,
//!    the material is not.
//!
//!  * Bits 0-2 contain the texture index ranging from 0 to 7.
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

use ray::{MIntersection, MRay};
use simd::{Mask, Mf32};
use vector3::MVector3;

struct MaterialBank;

impl MaterialBank {

    /// Returns the sky color for a ray in the given direction.
    pub fn sky_intensity(ray_direction: MVector3) -> MVector3 {
        // TODO: Better sky model.
        let up = MVector3::new(Mf32::zero(), Mf32::one(), Mf32::one());
        let half = Mf32::broadcast(0.5);
        let d = ray_direction.dot(up).mul_add(half, half);
        let r = Mf32::broadcast(1.0) * d;
        let g = Mf32::broadcast(1.0) * (d * d);
        let b = Mf32::broadcast(1.0) * d * (d * d);
        MVector3::new(r, g, b)
    }

    /// Continues the path of a photon.
    ///
    /// If a surface intersected a material with the specified material index,
    /// at the given position with the given ray direction, then this will
    /// compute the ray that continues the light path. If the material is
    /// emissive the path does not continue, so a mask is also returned that is
    /// set to ones for paths that need to continue, and zeroes where the
    /// material is emissive. A factor to multiply the final color by is
    /// returned as well.
    pub fn continue_path(material: Mf32,
                         isect: &MIntersection,
                         ray_direction: MVector3)
                         -> (MVector3, Mask, MRay) {
        // The most significant bit of `material` determines whether the
        // material is emissive. If it is, then the light path ends here.
        let mask = Mask::ones().pick(material, Mf32::zero());

        // TODO: Do a diffuse bounce and use a proper material.
        // For now, do a specular reflection.

        let dot = isect.normal.dot(ray_direction);
        let direction = isect.normal.mul_add(dot + dot, ray_direction);

        // Build a new ray, offset by an epsilon from the intersection so we
        // don't intersect the same surface again.
        let origin = direction.mul_add(Mf32::epsilon(), isect.position);
        let new_ray = MRay::new(origin, direction);

        let white = MVector3::new(Mf32::one(), Mf32::one(), Mf32::one());

        (white, mask, new_ray)
    }
}