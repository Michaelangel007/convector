//! Implements a bounding volume hierarchy.

use aabb::Aabb;
use ray::{MIntersection, MRay};
use simd::{Mask, Mf32};
use triangle::Triangle;
use util;
use vector3::{Axis, SVector3};
use wavefront::Mesh;

/// One node in a bounding volume hierarchy.
struct BvhNode {
    aabb: Aabb,

    /// For leaf nodes, the index of the first triangle, for internal nodes, the
    /// index of the first child. The second child is at `index + 1`.
    index: u32,

    /// For leaf nodes, the number of triangle, zero for internal nodes.
    len: u32,
}

/// A bounding volume hierarchy.
pub struct Bvh {
    nodes: Vec<BvhNode>,
    triangles: Vec<Triangle>,
}

/// Reference to a triangle used during BVH construction.
#[derive(Debug)]
struct TriangleRef {
    aabb: Aabb,
    barycenter: SVector3,
    index: usize,
}

/// A node used during BVH construction.
struct InterimNode {
    /// Bounding box of the triangles in the node.
    outer_aabb: Aabb,

    /// Bounding box of the barycenters of the triangles in the node.
    inner_aabb: Aabb,

    children: Vec<InterimNode>,
    triangles: Vec<TriangleRef>,
}

struct Bin<'a> {
    triangles: Vec<&'a TriangleRef>,
    aabb: Option<Aabb>,
}

trait Heuristic {
    /// Given that a ray has intersected the parent bounding box, estimates the
    /// cost of intersecting the child bounding box and the triangles in it.
    fn aabb_cost(&self, parent_aabb: &Aabb, aabb: &Aabb, num_tris: usize) -> f32;

    /// Estimates the cost of intersecting the given number of triangles.
    fn tris_cost(&self, num_tris: usize) -> f32;
}

struct SurfaceAreaHeuristic {
    aabb_intersection_cost: f32,
    triangle_intersection_cost: f32,
}

/// My own improvement over the classic surface area heuristic. See `aabb_cost`
/// implementation for more details.
struct TreeSurfaceAreaHeuristic {
    aabb_intersection_cost: f32,
    triangle_intersection_cost: f32,
    intersection_probability: f32,
}

impl TriangleRef {
    fn from_triangle(index: usize, tri: &Triangle) -> TriangleRef {
        TriangleRef {
            aabb: Aabb::enclose_points(&[tri.v0, tri.v1, tri.v2]),
            barycenter: tri.barycenter(),
            index: index,
        }
    }
}

impl<'a> Bin<'a> {
    fn new() -> Bin<'a> {
        Bin {
            triangles: Vec::new(),
            aabb: None,
        }
    }

    pub fn push(&mut self, tri: &'a TriangleRef) {
        self.triangles.push(tri);
        self.aabb = match self.aabb {
            Some(ref aabb) => Some(Aabb::enclose_aabbs(&[aabb.clone(), tri.aabb.clone()])),
            None => Some(tri.aabb.clone()),
        };
    }
}

impl InterimNode {
    /// Create a single node containing all of the triangles.
    fn from_triangle_refs(trirefs: Vec<TriangleRef>) -> InterimNode {
        InterimNode {
            outer_aabb: Aabb::enclose_aabbs(trirefs.iter().map(|tr| &tr.aabb)),
            inner_aabb: Aabb::enclose_points(trirefs.iter().map(|tr| &tr.barycenter)),
            children: Vec::new(),
            triangles: trirefs,
        }
    }

    fn inner_aabb_origin_and_size(&self, axis: Axis) -> (f32, f32) {
        let min = self.inner_aabb.origin.get_coord(axis);
        let max = self.inner_aabb.far.get_coord(axis);
        let size = max - min;
        (min, size)
    }

    /// Puts triangles into bins along the specified axis.
    fn bin_triangles<'a>(&'a self, bins: &mut [Bin<'a>], axis: Axis) {
        // Compute the bounds of the bins.
        let (min, size) = self.inner_aabb_origin_and_size(axis);

        // Put the triangles in bins.
        for tri in &self.triangles {
            let coord = tri.barycenter.get_coord(axis);
            let index = ((bins.len() as f32) * (coord - min) / size).floor() as usize;
            let index = if index < bins.len() { index } else { bins.len() - 1 };
            bins[index].push(tri);

            // If a lot of geometry ends up in one bin, binning is
            // apparently not effective.
            let num_tris = self.triangles.len();
            if bins[index].triangles.len() > num_tris / 8 && num_tris > bins.len() {
                println!("warning: triangle distribution is very non-uniform");
                println!("         binning will not be effective");
                println!("         number of triangles: {}", num_tris);
            }
        }
    }

    /// Returs the bounding box enclosing the bin bounding boxes.
    fn enclose_bins(bins: &[Bin]) -> Aabb {
        let aabbs = bins.iter()
                        .filter(|bin| bin.triangles.len() > 0)
                        .map(|bin| bin.aabb.as_ref().unwrap());

        Aabb::enclose_aabbs(aabbs)
    }

    /// Returns whether there is more than one non-empty bin.
    fn are_bins_valid(bins: &[Bin]) -> bool {
        1 < bins.iter().filter(|bin| !bin.triangles.is_empty()).count()
    }

    /// Returns the bin index such that for the cheapest split, all bins with a
    /// lower index should go into one node. Also returns the cost of the split.
    fn find_cheapest_split<H>(&self, heuristic: &H, bins: &[Bin]) -> (usize, f32) where H: Heuristic {
        let mut best_split_at = 0;
        let mut best_split_cost = 0.0;
        let mut is_first = true;

        // Consiter every split position after the first non-empty bin, until
        // right before the last non-empty bin.
        let first = bins.iter().position(|bin| !bin.triangles.is_empty()).unwrap() + 1;
        let last = bins.iter().rposition(|bin| !bin.triangles.is_empty()).unwrap();

        for i in first..last {
            let left_bins = &bins[..i];
            let left_aabb = InterimNode::enclose_bins(left_bins);
            let left_count = left_bins.iter().map(|b| b.triangles.len()).sum();

            let right_bins = &bins[i..];
            let right_aabb = InterimNode::enclose_bins(right_bins);
            let right_count = left_bins.iter().map(|b| b.triangles.len()).sum();

            let left_cost = heuristic.aabb_cost(&self.outer_aabb, &left_aabb, left_count);
            let right_cost = heuristic.aabb_cost(&self.outer_aabb, &right_aabb, right_count);
            let cost = left_cost + right_cost;

            if cost < best_split_cost || is_first {
                best_split_cost = cost;
                best_split_at = i;
                is_first = false;
            }
        }

        (best_split_at, best_split_cost)
    }

    /// Splits the node if that is would be beneficial according to the
    /// heuristic.
    fn split<H>(&mut self, heuristic: &H) where H: Heuristic {
        // If there is only one triangle, splitting does not make sense.
        if self.triangles.len() <= 1 {
            return
        }

        let mut best_split_axis = Axis::X;
        let mut best_split_at = 0.0;
        let mut best_split_cost = 0.0;
        let mut is_first = true;

        // Find the cheapest split.
        for &axis in &[Axis::X, Axis::Y, Axis::Z] {
            let mut bins: Vec<Bin> = (0..64).map(|_| Bin::new()).collect();

            self.bin_triangles(&mut bins, axis);

            if InterimNode::are_bins_valid(&bins) {
                let (index, cost) = self.find_cheapest_split(heuristic, &bins);

                if cost < best_split_cost || is_first {
                    let (min, size) = self.inner_aabb_origin_and_size(axis);
                    best_split_axis = axis;
                    best_split_at = min + size / (bins.len() as f32) * (index as f32);
                    best_split_cost = cost;
                    is_first = false;
                }
            } else {
                // Consider a different splitting strategy?
            }
        }

        // Something must have set the cost.
        assert!(!is_first);

        // Do not split if the split node is more expensive than the unsplit
        // one.
        let no_split_cost = heuristic.tris_cost(self.triangles.len());
        if no_split_cost < best_split_cost {
            return
        }

        // Partition the triangles into two child nodes.
        let pred = |tri: &TriangleRef| tri.barycenter.get_coord(best_split_axis) <= best_split_at;
        // TODO: remove type annotation.
        let (left_tris, right_tris): (Vec<_>, Vec<_>) = self.triangles.drain(..).partition(pred);

        // It can happen that the best split is not to split at all ... BUT in
        // that case the no split cost should be lower than the all-in-one-side
        // cost ... so this should not occur.
        if left_tris.is_empty() || right_tris.is_empty() {
            println!("one of the sides was empty!");
            println!("no split cost: {}", no_split_cost);
            println!("best split cost: {}", best_split_cost);
            println!("split at: {} on {:?} axis", best_split_at, best_split_axis);
            println!("left tris: {:?}", left_tris);
            println!("right tris: {:?}", right_tris);
        }

        let left = InterimNode::from_triangle_refs(left_tris);
        let right = InterimNode::from_triangle_refs(right_tris);

        // TODO: Perhaps make child with biggest surface area go first.
        self.children.push(left);
        self.children.push(right);
    }

    /// Recursively splits the node, constructing the BVH.
    fn split_recursive<H>(&mut self, heuristic: &H) where H: Heuristic {
        // TODO: This would be an excellent candidate for Rayon I think.
        self.split(heuristic);
        for child_node in &mut self.children {
            child_node.split_recursive(heuristic);
        }
    }

    /// Returns the number of triangle refs in the leaves.
    fn count_triangles(&self) -> usize {
        let child_tris: usize = self.children.iter().map(|ch| ch.count_triangles()).sum();
        let self_tris = self.triangles.len();
        child_tris + self_tris
    }

    /// Returns the number of nodes in the BVH, including self.
    fn count_nodes(&self) -> usize {
        let child_count: usize = self.children.iter().map(|ch| ch.count_nodes()).sum();
        1 + child_count
    }

    /// Returns the number of leaf nodes in the BVH.
    fn count_leaves(&self) -> usize {
        let leaf_count: usize = self.children.iter().map(|ch| ch.count_leaves()).sum();
        let self_leaf = if self.children.is_empty() { 1 } else { 0 };
        self_leaf + leaf_count
    }

    /// Returns the ratio of the parent area to the child node area,
    /// summed over all the nodes.
    fn summed_area_ratio(&self) -> f32 {
        let child_contribution: f32 = self.children.iter().map(|ch| ch.summed_area_ratio()).sum();
        let self_area = self.outer_aabb.area();
        let child_sum: f32 = self.children.iter().map(|ch| ch.outer_aabb.area() / self_area).sum();
        child_sum + child_contribution
    }

    /// Converts the interim representation that was useful for building the BVH
    /// into a representation that is optimized for traversing the BVH.
    fn crystallize(&self,
                   source_triangles: &[Triangle],
                   nodes: &mut Vec<BvhNode>,
                   sorted_triangles: &mut Vec<Triangle>,
                   into_index: usize) {
        // Nodes must always be pushed in pairs to keep siblings on the same
        // cache line.
        assert_eq!(0, nodes.len() % 2);

        nodes[into_index].aabb = self.outer_aabb.clone();

        if self.triangles.is_empty() {
            // This is an internal node.
            assert_eq!(2, self.children.len());

            // Allocate two new nodes for the children.
            let child_index = nodes.len();
            nodes.push(BvhNode::new());
            nodes.push(BvhNode::new());

            // Recursively crystallize the child nodes.
            // TODO: Order by surface area.
            self.children[0].crystallize(source_triangles, nodes, sorted_triangles, child_index + 0);
            self.children[1].crystallize(source_triangles, nodes, sorted_triangles, child_index + 1);

            nodes[into_index].index = child_index as u32;
            nodes[into_index].len = 0;
        } else {
            // This is a leaf node.
            assert_eq!(0, self.children.len());

            nodes[into_index].index = sorted_triangles.len() as u32;
            nodes[into_index].len = self.triangles.len() as u32;

            // Copy the triangles into the triangle buffer.
            let tris = self.triangles.iter().map(|triref| source_triangles[triref.index].clone());
            sorted_triangles.extend(tris);
        }
    }
}

impl Heuristic for SurfaceAreaHeuristic {
    fn aabb_cost(&self, parent_aabb: &Aabb, aabb: &Aabb, num_tris: usize) -> f32 {
        // We are certainly going to intersect the child AABB, so pay the full
        // price for that.
        let fixed_cost = self.aabb_intersection_cost;

        // Without further information, the best guess for the probability
        // that the bounding box was hit, given that the parent was already
        // intersected, is the ratio of their areas.
        let ac_ap = aabb.area() / parent_aabb.area();

        // We have to test all of the triangles, but only if the bounding box
        // was intersected, so weigh with the probability.
        fixed_cost + ac_ap * self.tris_cost(num_tris).log2()
    }

    fn tris_cost(&self, num_tris: usize) -> f32 {
        (num_tris as f32) * self.triangle_intersection_cost
    }
}

impl Heuristic for TreeSurfaceAreaHeuristic {
    fn aabb_cost(&self, parent_aabb: &Aabb, aabb: &Aabb, num_tris: usize) -> f32 {
        // The SAH adds the cost of intersecting all the triangles, but for a
        // non-leaf node, it is rarely the case that they all will be
        // intersected. Instead, assume that the triangles are organized into a
        // balanced BVH with two triangles per leaf. If you work out the math
        // (see pdf), the following expression is what comes out:

        let ac_ap = aabb.area() / parent_aabb.area();
        let p = self.intersection_probability;
        let n = num_tris as f32;
        let m = n.log2();

        let aabb_term = 1.0 + ac_ap * (2.0 * p - n * p.powf(m)) / (p - 2.0 * p * p);
        let tri_term = n * p.powf(m - 1.0) * ac_ap;

        aabb_term * self.aabb_intersection_cost + tri_term * self.triangle_intersection_cost
    }

    fn tris_cost(&self, num_tris: usize) -> f32 {
        (num_tris as f32) * self.triangle_intersection_cost
    }
}

impl BvhNode {
    /// Returns a zeroed node, to be filled later.
    fn new() -> BvhNode {
        BvhNode {
            aabb: Aabb::zero(),
            index: 0,
            len: 0,
        }
    }
}

impl Bvh {
    pub fn build(source_triangles: &[Triangle]) -> Bvh {
        println!("building bvh ...");
        // Actual triangles are not important to the BVH, convert them to AABBs.
        let trirefs = (0..).zip(source_triangles.iter())
                           .map(|(i, tri)| TriangleRef::from_triangle(i, tri))
                           .collect();

        let mut root = InterimNode::from_triangle_refs(trirefs);

        // The values here are based on benchmarks. You can run `make bench` to
        // run these benchmarks. By plugging in the results for your rig you
        // might be able to achieve slightly better performance.
        let heuristic = TreeSurfaceAreaHeuristic {
            aabb_intersection_cost: 40.0,
            triangle_intersection_cost: 120.0,
            intersection_probability: 0.1,
        };

        // Build the BVH of interim nodes.
        root.split_recursive(&heuristic);

        // There should be at least one split, because crystallized nodes are
        // stored in pairs. There is no single root, there are two roots. (Or,
        // the root is implicit and its bounding box is infinite, if you like.)
        assert_eq!(2, root.children.len());

        // Allocate one buffer for the BVH nodes and one for the triangles. For
        // better data locality, the source triangles are reordered. Also, a
        // triangle might be included in multiple nodes. In that case it is
        // simply duplicated in the new buffer. The node buffer is aligned to a
        // cache line: nodes are always accessed in pairs, and one pair fits
        // exactly in one cache line.
        let num_tris = root.count_triangles();
        let num_nodes = root.count_nodes();
        let mut nodes = util::cache_line_aligned_vec(num_nodes);
        let mut sorted_triangles = Vec::with_capacity(num_tris);

        println!("done constructing bvh, crystallizing ...");

        // Write the tree of interim nodes that is all over the heap currently,
        // neatly packed into the buffers that we just allocated.
        let left = &root.children[0];
        let right = &root.children[1];
        nodes.push(BvhNode::new());
        nodes.push(BvhNode::new());
        // TODO: Order these by area.
        left.crystallize(&source_triangles, &mut nodes, &mut sorted_triangles, 0);
        right.crystallize(&source_triangles, &mut nodes, &mut sorted_triangles, 1);

        // Print some statistics about the BVH:
        let num_leaves = root.count_leaves();
        let tris_per_leaf = (num_tris as f32) / (num_leaves as f32);
        let area_ratio_sum = root.summed_area_ratio();
        let avg_area_ratio = area_ratio_sum / (num_nodes as f32);
        println!("bvh statistics:");
        println!("  average triangles per leaf: {:0.2}", tris_per_leaf);
        println!("  average child area / parent area: {:0.2}", avg_area_ratio);

        Bvh {
            nodes: nodes,
            triangles: sorted_triangles,
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

        Bvh::build(&triangles)
    }

    pub fn intersect_nearest(&self, ray: &MRay, mut isect: MIntersection) -> MIntersection {
        // Keep a stack of nodes that still need to be intersected. This does
        // involve a heap allocation, but that is not so bad. Using a small
        // on-stack vector from the smallvec crate (which falls back to heap
        // allocation if it grows) actually reduced performance by about 5 fps.
        // If there is an upper bound on the BVH depth, then perhaps manually
        // rolling an on-stack (memory) stack (data structure) could squeeze out
        // a few more fps.
        let mut stack = Vec::with_capacity(10);

        // A note about `get_unchecked`: array indexing in Rust is checked by
        // default, but by construction all indices in the BVH are valid, so
        // let's not waste instructions on those bounds checks.

        let root_0 = unsafe { self.nodes.get_unchecked(0) };
        let root_1 = unsafe { self.nodes.get_unchecked(1) };
        let root_isect_0 = root_0.aabb.intersect(ray);
        let root_isect_1 = root_1.aabb.intersect(ray);

        if root_isect_0.any() {
            stack.push((root_isect_0, root_0));
        }
        if root_isect_1.any() {
            stack.push((root_isect_1, root_1));
        }

        while let Some((aabb_isect, node)) = stack.pop() {
            // If the AABB is further away than the current nearest
            // intersection, then nothing inside the node can yield
            // a closer intersection, so we can skip the node.
            if aabb_isect.is_further_away_than(isect.distance) {
                continue
            }

            if node.len == 0 {
                // This is an internal node.
                let child_0 = unsafe { self.nodes.get_unchecked(node.index as usize + 0) };
                let child_1 = unsafe { self.nodes.get_unchecked(node.index as usize + 1) };
                let child_isect_0 = child_0.aabb.intersect(ray);
                let child_isect_1 = child_1.aabb.intersect(ray);

                // TODO: Order by distance?
                if child_isect_0.any() {
                    stack.push((child_isect_0, child_0));
                }
                if child_isect_1.any() {
                    stack.push((child_isect_1, child_1));
                }
            } else {
                for i in node.index..node.index + node.len {
                    let triangle = unsafe { self.triangles.get_unchecked(i as usize) };
                    isect = triangle.intersect(ray, isect);
                }
            }
        }

        isect
    }

    pub fn intersect_any(&self, ray: &MRay, max_dist: Mf32) -> Mask {
        // This is actually just doing a full BVH intersection. I tried to do an
        // early out here; stop when all rays intersect at least something,
        // instead of finding the nearest intersection, but I could not measure
        // a performance improvement. `intersect_nearest` does try very hard not
        // to intersect more than necessary, and apparently that is good enough
        // already.
        let isect = MIntersection {
            position: ray.direction.mul_add(max_dist, ray.origin),
            normal: ray.direction,
            distance: max_dist,
        };
        let isect = self.intersect_nearest(ray, isect);
        isect.distance.geq(max_dist - Mf32::epsilon())
    }
}
