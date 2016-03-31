//! Implements quaternion utilities to handle rotation.

use simd::Mf32;
use vector3::MVector3;

#[cfg(test)]
use bench;

#[derive(Copy, Clone, Debug)]
pub struct SQuaternion {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

pub struct MQuaternion {
    pub a: Mf32,
    pub b: Mf32,
    pub c: Mf32,
    pub d: Mf32,
}

impl SQuaternion {
    pub fn new(a: f32, b: f32, c: f32, d: f32) -> SQuaternion {
        SQuaternion {
            a: a,
            b: b,
            c: c,
            d: d,
        }
    }
}

impl MQuaternion {
    pub fn broadcast(q: SQuaternion) -> MQuaternion {
        MQuaternion {
            a: Mf32::broadcast(q.a),
            b: Mf32::broadcast(q.b),
            c: Mf32::broadcast(q.c),
            d: Mf32::broadcast(q.d),
        }
    }
}

pub fn rotate(vector: &MVector3, rotation: &MQuaternion) -> MVector3 {
    let v = vector;
    let q = rotation;

    // For a unit quaternion q and a vector in R3 identified with the subspace
    // of the quaternion algebra spanned by (i, j, k), the rotated vector is
    // given by q * v * q^-1. (And because q is a unit quaternion, its inverse
    // is its conjugate.) This means that we can compute the rotation in two
    // steps: p = v * q^-1, and q * p. The first step is simpler than generic
    // quaternion multiplication because we know that v is pure imaginary. The
    // second step simpler than generc quaternion multiplication because we know
    // that the result is pure imaginary, so the real component does not have to
    // be computed.

    // For q = a + b*i + c*j + d*k and v = x*i + y*j + c*z, v * q^-1 is given
    // by
    //
    //     b*x + c*y + d*z +
    //     ((a - b)*x + (c - d)*(y + z) + b*x - c*y + d*z)*i +
    //     (d*x + a*y - b*z)*j +
    //     (-(c + d)*x + (a + b)*(y + z) + d*x - a*y - b*z)*k
    //
    // I did not bother with using `mul_add` or eliminating common
    // subexpressions below because the code is unreadable enough as it is ...

    let pa = q.b * v.x + q.c * v.y + q.d * v.z;
    let pb = q.b * v.x - q.c * v.y + q.d * v.z + (q.a - q.b) * v.x + (q.c - q.d) * (v.y + v.z);
    let pc = q.d * v.x + q.a * v.y - q.b * v.z;
    let pd = q.d * v.x - q.a * v.y - q.b * v.z - (q.c + q.d) * v.x + (q.a + q.b) * (v.y + v.z);

    // The product of q = qa + qb*i + qc*j + qd*k and
    // p = pa + pb*i + pc*j + pd*k is given by
    //
    //    pa*qa - pb*qb - pc*qc - pd*qd +
    //    ((pa + pb)*(qa + qb) - (pc - pd)*(qc + qd) - pa*qa - pb*qb + pc*qc - pd*qd)*i +
    //    (pc*qa - pd*qb + pa*qc + pb*qd)*j +
    //    ((pc + pd)*(qa + qb) + (pa - pb)*(qc + qd) - pc*qa - pd*qb - pa*qc + pb*qd)*k

    let rb = (pa + pb) * (q.a + q.b) - (pc - pd) * (q.c + q.d) - pa * q.a - pb * q.b + pc * q.c - pd * q.d;
    let rc = pc * q.a - pd * q.b + pa * q.c + pb * q.d;
    let rd = (pc + pd) * (q.a + q.b) + (pa - pb) * (q.c + q.d) - pc * q.a - pd * q.b - pa * q.c + pb * q.d;

    MVector3::new(rb, rc, rd)
}

#[cfg(test)]
fn assert_mvectors_equal(expected: MVector3, computed: MVector3, margin: f32) {
    // Test that the vectors are equal, to within floating point inaccuracy
    // margins.
    let error = (computed - expected).norm_squared();
    assert!((Mf32::broadcast(margin * margin) - error).all_sign_bits_positive(),
            "expected: ({}, {}, {}), computed: ({}, {}, {})",
            expected.x.0, expected.y.0, expected.z.0,
            computed.x.0, computed.y.0, computed.z.0);
}

#[test]
fn rotate_identity() {
    let identity = SQuaternion::new(1.0, 0.0, 0.0, 0.0);
    let vectors = bench::points_on_sphere_m(32);
    for v in &vectors {
        assert_mvectors_equal(*v, rotate(v, &MQuaternion::broadcast(identity)), 1e-7);
    }
}

#[test]
fn rotate_x() {
    let half_sqrt_2 = 0.5 * 2.0_f32.sqrt();
    let rotation = SQuaternion::new(half_sqrt_2, half_sqrt_2, 0.0, 0.0);
    let vectors = bench::points_on_sphere_m(32);
    for v in &vectors {
        // Rotate the vector by pi/2 radians around the x-axis. This is
        // equivalent to y <- -z, z <- y, so compute the rotation in two
        // different ways, and verify that the result is the same to within the
        // floating point inaccuracy margin.
        let computed = rotate(v, &MQuaternion::broadcast(rotation));
        let expected = MVector3::new(v.x, -v.z, v.y);
        assert_mvectors_equal(expected, computed, 1e-6);
    }
}

#[test]
fn rotate_y() {
    let half_sqrt_2 = 0.5 * 2.0_f32.sqrt();
    let rotation = SQuaternion::new(half_sqrt_2, 0.0, half_sqrt_2, 0.0);
    let vectors = bench::points_on_sphere_m(32);
    for v in &vectors {
        // Rotate the vector by pi/2 radians around the y-axis. This is
        // equivalent to x <- z, z <- -x, so compute the rotation in two
        // different ways, and verify that the result is the same to within the
        // floating point inaccuracy margin.
        let computed = rotate(v, &MQuaternion::broadcast(rotation));
        let expected = MVector3::new(v.z, v.y, -v.x);
        assert_mvectors_equal(expected, computed, 1e-6);
    }
}

#[test]
fn rotate_z() {
    let half_sqrt_2 = 0.5 * 2.0_f32.sqrt();
    let rotation = SQuaternion::new(half_sqrt_2, 0.0, 0.0, half_sqrt_2);
    let vectors = bench::points_on_sphere_m(32);
    for v in &vectors {
        // Rotate the vector by pi/2 radians around the y-axis. This is
        // equivalent to y <- x, x <- -y, so compute the rotation in two
        // different ways, and verify that the result is the same to within the
        // floating point inaccuracy margin.
        let computed = rotate(v, &MQuaternion::broadcast(rotation));
        let expected = MVector3::new(-v.y, v.x, v.z);
        assert_mvectors_equal(expected, computed, 1e-6);
    }
}
