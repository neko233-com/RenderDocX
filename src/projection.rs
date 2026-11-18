//! Projection matrix construction and unprojection for the mesh viewer,
//! mirroring qrenderdoc's use of `MeshDisplayConfig` (fov/aspect/near/far/ortho)
//! to unproject rasterized NDC coordinates back into world space.
//!
//! Pure Rust math; no bridge or GPU required to compute the matrices.

/// Column-major 4x4 matrix (same layout as GLSL/HLSL constant buffers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub m: [f32; 16],
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4 {
        m: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    };

    /// Multiply this matrix by a column vector, returning the result.
    pub fn transform(self, v: [f32; 4]) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        for row in 0..4 {
            out[row] = self.m[row] * v[0]
                + self.m[4 + row] * v[1]
                + self.m[8 + row] * v[2]
                + self.m[12 + row] * v[3];
        }
        out
    }

    /// Gauss-Jordan inverse. Returns `None` for a singular matrix.
    pub fn inverse(self) -> Option<Mat4> {
        let mut a = self.m;
        let mut inv = Mat4::IDENTITY.m;
        for col in 0..4 {
            let pivot_row = (col..4).max_by(|&r1, &r2| {
                a[r1 * 4 + col]
                    .abs()
                    .partial_cmp(&a[r2 * 4 + col].abs())
                    .unwrap()
            })?;
            if a[pivot_row * 4 + col].abs() < 1e-12 {
                return None;
            }
            if pivot_row != col {
                for k in 0..4 {
                    a.swap(col * 4 + k, pivot_row * 4 + k);
                    inv.swap(col * 4 + k, pivot_row * 4 + k);
                }
            }
            let pivot = a[col * 4 + col];
            for k in 0..4 {
                a[col * 4 + k] /= pivot;
                inv[col * 4 + k] /= pivot;
            }
            for row in 0..4 {
                if row == col {
                    continue;
                }
                let factor = a[row * 4 + col];
                for k in 0..4 {
                    a[row * 4 + k] -= factor * a[col * 4 + k];
                    inv[row * 4 + k] -= factor * inv[col * 4 + k];
                }
            }
        }
        Some(Mat4 { m: inv })
    }
}

/// Perspective projection matrix (D3D-style, z in [0,1]).
pub fn perspective(fov_degrees: f32, aspect: f32, near: f32, far: f32) -> Option<Mat4> {
    if aspect <= 0.0 || near <= 0.0 || far <= near {
        return None;
    }
    let f = 1.0 / (fov_degrees.to_radians() * 0.5).tan();
    let mut m = [0.0f32; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = far / (far - near);
    m[11] = 1.0;
    m[14] = -(near * far) / (far - near);
    Some(Mat4 { m })
}

/// Infinite far-plane (reverse-Z friendly) perspective matrix.
pub fn perspective_infinite(fov_degrees: f32, aspect: f32, near: f32) -> Option<Mat4> {
    if aspect <= 0.0 || near <= 0.0 {
        return None;
    }
    let f = 1.0 / (fov_degrees.to_radians() * 0.5).tan();
    let mut m = [0.0f32; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = 1.0;
    m[11] = 1.0;
    m[14] = -near;
    Some(Mat4 { m })
}

/// Orthographic projection matrix (D3D-style, z in [0,1]).
pub fn orthographic(width: f32, height: f32, near: f32, far: f32) -> Option<Mat4> {
    if width <= 0.0 || height <= 0.0 || far <= near {
        return None;
    }
    let mut m = Mat4::IDENTITY.m;
    m[0] = 2.0 / width;
    m[5] = 2.0 / height;
    m[10] = 1.0 / (far - near);
    m[14] = -near / (far - near);
    Some(Mat4 { m })
}

/// Unproject an NDC point through the inverse of a projection matrix.
pub fn unproject(ndc: [f32; 3], inverse_projection: Mat4) -> [f32; 3] {
    let p = inverse_projection.transform([ndc[0], ndc[1], ndc[2], 1.0]);
    let w = p[3];
    if w.abs() < 1e-12 {
        return [0.0, 0.0, 0.0];
    }
    [p[0] / w, p[1] / w, p[2] / w]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_transform_and_inverse() {
        assert_eq!(
            Mat4::IDENTITY.transform([1.0, 2.0, 3.0, 1.0]),
            [1.0, 2.0, 3.0, 1.0]
        );
        assert_eq!(Mat4::IDENTITY.inverse(), Some(Mat4::IDENTITY));
    }

    #[test]
    fn perspective_unproject_returns_ndc_space() {
        let proj = perspective(90.0, 1.0, 0.1, 100.0).unwrap();
        let inv = proj.inverse().unwrap();
        let world = unproject([0.0, 0.0, 0.5], inv);
        // center NDC ray maps back to x=y=0, some view-space depth.
        assert!(world[0].abs() < 1e-4 && world[1].abs() < 1e-4);
        assert!(world[2] > 0.0);
        // round-trip through projection returns the NDC point.
        let back = proj.transform([world[0], world[1], world[2], 1.0]);
        assert!((back[0] / back[3]).abs() < 1e-4);
    }

    #[test]
    fn perspective_infinite_and_orthographic_are_invertible() {
        assert!(perspective_infinite(75.0, 16.0 / 9.0, 0.5)
            .unwrap()
            .inverse()
            .is_some());
        assert!(orthographic(2.0, 2.0, 0.1, 10.0)
            .unwrap()
            .inverse()
            .is_some());
        assert!(perspective(90.0, 0.0, 0.1, 10.0).is_none());
        assert!(orthographic(2.0, 2.0, 5.0, 1.0).is_none());
    }
}
