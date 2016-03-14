//! Implements a bounding volume hierarchy.

use aabb::Aabb;
use geometry::Triangle;
use ray::{MIntersection, MRay};
use simd::{Mask, Mf32};
use std::cmp::PartialOrd;
use vector3::{Axis, SVector3};
use wavefront::Mesh;

/// One node in a bounding volume hierarchy.
struct BvhNode {
    aabb: Aabb,
    children: Vec<BvhNode>,
    geometry: Vec<Triangle>,
}

/// A bounding volume hierarchy.
pub struct Bvh {
    root: BvhNode,
}

fn build_bvh_node(triangles: &mut [Triangle]) -> BvhNode {
    let mut aabb = Aabb::new(SVector3::zero(), SVector3::zero());

    // Compute the bounding box that encloses all triangles.
    for triangle in triangles.iter() {
        aabb = Aabb::enclose_aabbs(&aabb, &triangle.aabb);
    }

    let centroids: Vec<SVector3> = triangles.iter().map(|tri| tri.aabb.center()).collect();
    let centroid_aabb = Aabb::enclose_points(&centroids[..]);

    // Ideally every node would contain two triangles, so splitting less than
    // four triangles does not make sense; make a leaf node in that case.
    if triangles.len() < 4 {
        return BvhNode {
            aabb: aabb,
            children: Vec::new(),
            geometry: triangles.iter().cloned().collect(),
        }
    }

    // Split along the axis in which the box is largest.
    let mut size = centroid_aabb.size.x;
    let mut axis = Axis::X;

    if centroid_aabb.size.y > size {
        size = centroid_aabb.size.y;
        axis = Axis::Y;
    }

    if centroid_aabb.size.z > size {
        size = centroid_aabb.size.z;
        axis = Axis::Z;
    }

    // Sort the  triangles along that axis (panic on NaN).
    triangles.sort_by(|a, b| PartialOrd::partial_cmp(
        &a.barycenter().get_coord(axis),
        &b.barycenter().get_coord(axis)).unwrap());

    let half_way = centroid_aabb.origin.get_coord(axis) + size * 0.5;

    // Find the index to split at so that everything before the split has
    // coordinate less than `half_way` and everything after has larger or equal
    // coordinates.
    let mut split_point = triangles.binary_search_by(|tri| {
        PartialOrd::partial_cmp(&tri.aabb.center().get_coord(axis), &half_way).unwrap()
    }).unwrap_or_else(|idx| idx);

    // Ensure a balanced tree at the leaves.
    // (This also ensures that the recursion terminates.)
    if split_point > triangles.len() - 2 {
        split_point = triangles.len() - 2;
    }
    if split_point < 2 {
        split_point = 2;
    }

    let (left_triangles, right_triangles) = triangles.split_at_mut(split_point);
    let left_node = build_bvh_node(left_triangles);
    let right_node = build_bvh_node(right_triangles);
    BvhNode {
        aabb: aabb,
        children: vec![left_node, right_node],
        geometry: Vec::new(),
    }
}

impl Bvh {
    pub fn build(mut triangles: Vec<Triangle>) -> Bvh {
        // TODO: Use rayon for data parallelism here.
        let root = build_bvh_node(&mut triangles);
        Bvh {
            root: root,
        }
    }

    pub fn from_meshes(meshes: &[Mesh]) -> Bvh {
        let mut triangles = Vec::new();

        for mesh in meshes {
            let mesh_triangles = mesh.triangles.iter().map(
                |&(i1, i2, i3)| {
                    let v1 = mesh.vertices[i1 as usize];
                    let v2 = mesh.vertices[i2 as usize];
                    let v3 = mesh.vertices[i3 as usize];
                    Triangle::new(v1, v2, v3)
                });
            triangles.extend(mesh_triangles);
        }

        Bvh::build(triangles)
    }

    pub fn intersect_nearest(&self, ray: &MRay, mut isect: MIntersection) -> MIntersection {
        // Keep a stack of nodes that still need to be intersected. This does
        // involve a heap allocation, but that is not so bad. Using a small
        // on-stack vector from the smallvec crate (which falls back to heap
        // allocation if it grows) actually reduced performance by about 5 fps.
        // If there is an upper bound on the BVH depth, then perhaps manually
        // rolling an on-stack (memory) stack (data structure) could squeeze out
        // a few more fps.
        let mut nodes = Vec::with_capacity(10);

        let root_isect = self.root.aabb.intersect(ray);
        if root_isect.any() {
            nodes.push((root_isect, &self.root));
        }

        while let Some((aabb_isect, node)) = nodes.pop() {
            // If the AABB is further away than the current nearest
            // intersection, then nothing inside the node can yield
            // a closer intersection, so we can skip the node.
            if aabb_isect.is_further_away_than(isect.distance) {
                continue
            }

            if node.geometry.is_empty() {
                for child in &node.children {
                    let child_isect = child.aabb.intersect(ray);
                    if child_isect.any() {
                        nodes.push((child_isect, child));
                    }
                }
            } else {
                for triangle in &node.geometry {
                    isect = triangle.intersect_full(ray, isect);
                }
            }
        }

        isect
    }

    pub fn intersect_any(&self, ray: &MRay, max_dist: Mf32) -> Mask {
        let isect = MIntersection {
            position: ray.direction.mul_add(max_dist, ray.origin),
            normal: ray.direction,
            distance: max_dist,
        };
        let isect = self.intersect_nearest(ray, isect);
        isect.distance.geq(max_dist - Mf32::epsilon())
    }
}
