//! Procedural 3-D meshes — vehicles, signs, poles, trees — plus surface-
//! conforming decal shadow quads. Pure geometry: no char masks or sprites.

use crate::config::*;
use crate::render::camera::norm3;
use crate::render::raster::{Material, Quad, Vertex};

/// A mesh in LOCAL space (origin at ground contact, forward = +Z).
#[derive(Clone)]
pub struct Mesh {
    pub quads: Vec<Quad>,
}

fn v(pos: [f32; 3], normal: [f32; 3]) -> Vertex {
    Vertex { pos, normal }
}

/// Add an axis-aligned box (skip bottom face) with per-face materials.
#[allow(clippy::too_many_arguments)]
fn add_box(m: &mut Mesh, min: [f32; 3], max: [f32; 3], mat: Material) {
    let c = |a: f32, b: f32| (a + b) * 0.5;
    let n = [
        norm3([1.0, 0.0, 0.0]),
        norm3([-1.0, 0.0, 0.0]),
        norm3([0.0, 1.0, 0.0]),
        norm3([0.0, 0.0, 1.0]),
        norm3([0.0, 0.0, -1.0]),
    ];
    // +X / −X
    for (sign, nm) in [(1.0f32, n[0]), (-1.0f32, n[1])] {
        let x = if sign > 0.0 { max[0] } else { min[0] };
        m.quads.push(Quad {
            v: [
                v([x, min[1], min[2]], nm),
                v([x, min[1], max[2]], nm),
                v([x, max[1], max[2]], nm),
                v([x, max[1], min[2]], nm),
            ],
            mat,
        });
    }
    // Top
    m.quads.push(Quad {
        v: [
            v([min[0], max[1], min[2]], n[2]),
            v([max[0], max[1], min[2]], n[2]),
            v([max[0], max[1], max[2]], n[2]),
            v([min[0], max[1], max[2]], n[2]),
        ],
        mat,
    });
    // +Z (front) / −Z (rear)
    for (sign, nm) in [(1.0f32, n[3]), (-1.0f32, n[4])] {
        let z = if sign > 0.0 { max[2] } else { min[2] };
        m.quads.push(Quad {
            v: [
                v([min[0], min[1], z], nm),
                v([max[0], min[1], z], nm),
                v([max[0], max[1], z], nm),
                v([min[0], max[1], z], nm),
            ],
            mat,
        });
    }
    let _ = c;
}

const CAR_L: f32 = 4.4;
const CAR_W: f32 = 1.85;

/// Vehicle shell: lofted body + cabin/glass + wheels + emissive lights.
/// (~40 quads — visually equivalent to a high-poly shell at braille res.)
pub fn build_car(body_idx: u8, braking: bool, oncoming: bool) -> Mesh {
    let body = PALETTE[body_idx as usize % PALETTE.len()];
    let paint = Material::opaque(body, 0.25, 0.6);
    let dark = Material::opaque(PALETTE[PAL_CAR_DARK as usize], 0.5, 0.3);
    let glass = Material::opaque(PALETTE[PAL_GLASS as usize], 0.05, 0.9);
    let tire = Material::opaque(PALETTE[PAL_TIRE as usize], 0.95, 0.0);
    let light_rgb = if oncoming {
        PALETTE[PAL_HEAD as usize]
    } else if braking {
        PALETTE[PAL_TAIL as usize]
    } else {
        [130, 34, 26]
    };
    let mut m = Mesh { quads: Vec::with_capacity(48) };

    // Body slab.
    add_box(
        &mut m,
        [-CAR_W * 0.5, 0.28, -CAR_L * 0.5],
        [CAR_W * 0.5, 0.82, CAR_L * 0.5],
        paint,
    );
    // Cabin with glass sides.
    add_box(
        &mut m,
        [-CAR_W * 0.42, 0.82, -CAR_L * 0.22],
        [CAR_W * 0.42, 1.38, CAR_L * 0.18],
        glass,
    );
    // Cabin roof strip.
    add_box(
        &mut m,
        [-CAR_W * 0.44, 1.30, -CAR_L * 0.24],
        [CAR_W * 0.44, 1.42, CAR_L * 0.20],
        paint,
    );
    // Bumpers.
    add_box(&mut m, [-CAR_W * 0.52, 0.30, -CAR_L * 0.52], [CAR_W * 0.52, 0.55, -CAR_L * 0.46], dark);
    add_box(&mut m, [-CAR_W * 0.52, 0.30, CAR_L * 0.46], [CAR_W * 0.52, 0.55, CAR_L * 0.52], dark);

    // Wheels: short prisms slightly outside the body line.
    let wz = CAR_L * 0.31;
    let wx = CAR_W * 0.5;
    for sx in [-1.0f32, 1.0] {
        for sz in [-1.0f32, 1.0] {
            add_box(
                &mut m,
                [sx * wx - 0.09 * sx.signum(), 0.0, sz * wz - 0.34],
                [sx * wx + 0.09 * sx.signum(), 0.62, sz * wz + 0.34],
                tire,
            );
        }
    }

    // Emissive lights front (+Z) and rear (−Z).
    let head = Material::emissive(PALETTE[PAL_HEAD as usize]);
    let tail = Material::emissive(light_rgb);
    for sx in [-1.0f32, 1.0] {
        let x0 = sx * CAR_W * 0.36 - 0.28;
        m.quads.push(Quad {
            v: [
                v([x0, 0.60, CAR_L * 0.505], [0.0, 0.0, 1.0]),
                v([x0 + 0.26, 0.60, CAR_L * 0.505], [0.0, 0.0, 1.0]),
                v([x0 + 0.26, 0.74, CAR_L * 0.505], [0.0, 0.0, 1.0]),
                v([x0, 0.74, CAR_L * 0.505], [0.0, 0.0, 1.0]),
            ],
            mat: head,
        });
        m.quads.push(Quad {
            v: [
                v([x0, 0.58, -CAR_L * 0.505], [0.0, 0.0, -1.0]),
                v([x0 + 0.30, 0.58, -CAR_L * 0.505], [0.0, 0.0, -1.0]),
                v([x0 + 0.30, 0.72, -CAR_L * 0.505], [0.0, 0.0, -1.0]),
                v([x0, 0.72, -CAR_L * 0.505], [0.0, 0.0, -1.0]),
            ],
            mat: tail,
        });
    }
    m
}

/// Curve-warning sign: pole + yellow plate with black core.
pub fn build_sign() -> Mesh {
    let pole_m = Material::opaque(PALETTE[PAL_POLE as usize], 0.7, 0.4);
    let plate_m = Material::opaque(PALETTE[PAL_SIGN_YELLOW as usize], 0.4, 0.2);
    let core_m = Material::opaque(PALETTE[PAL_SIGN_BLACK as usize], 0.6, 0.0);
    let mut m = Mesh { quads: Vec::with_capacity(15) };
    add_box(&mut m, [-0.05, 0.0, -0.05], [0.05, 2.6, 0.05], pole_m);
    add_box(&mut m, [-0.55, 1.45, -0.03], [0.55, 2.55, 0.03], plate_m);
    add_box(&mut m, [-0.40, 1.60, 0.03], [0.40, 2.40, 0.035], core_m);
    m
}

/// Roadside tree: trunk + stacked foliage slabs (reads as canopy at range).
pub fn build_tree(jitter: u32) -> Mesh {
    let trunk = Material::opaque(PALETTE[PAL_DIRT as usize], 0.9, 0.0);
    let leaf_a = Material::opaque(PALETTE[PAL_GRASS_DARK as usize], 0.85, 0.0);
    let leaf_b = Material::opaque(PALETTE[PAL_GRASS as usize], 0.85, 0.0);
    let j = (jitter % 7) as f32 * 0.13;
    let h = 3.2 + j;
    let r = 1.1 + (jitter % 5) as f32 * 0.11;
    let mut m = Mesh { quads: Vec::with_capacity(25) };
    add_box(&mut m, [-0.14, 0.0, -0.14], [0.14, h * 0.42, 0.14], trunk);
    add_box(&mut m, [-r, h * 0.35, -r], [r, h * 0.68, r], leaf_a);
    add_box(&mut m, [-r * 0.72, h * 0.62, -r * 0.72], [r * 0.72, h, r * 0.72], leaf_b);
    add_box(&mut m, [-r * 0.4, h, -r * 0.4], [r * 0.4, h * 1.16, r * 0.4], leaf_a);
    m
}

/// Rotate a local-space point by yaw and translate to world.
fn place(p: [f32; 3], yaw_sin: f32, yaw_cos: f32, x: f32, y: f32, z: f32) -> ([f32; 3], bool) {
    (
        [p[0] * yaw_cos + p[2] * yaw_sin + x, p[1] + y, -p[0] * yaw_sin + p[2] * yaw_cos + z],
        false,
    )
}

/// Transform a mesh into world space around ground point `(x,z)` facing yaw.
pub fn instance(mesh: &Mesh, x: f32, y: f32, z: f32, yaw: f32) -> Vec<Quad> {
    let (s, c) = yaw.sin_cos();
    mesh.quads
        .iter()
        .map(|q| {
            let mut out = Quad { mat: q.mat, v: q.v };
            for i in 0..4 {
                let (pos, _) = place(q.v[i].pos, s, c, x, y, z);
                let n = q.v[i].normal;
                out.v[i].pos = pos;
                out.v[i].normal = [n[0] * c + n[2] * s, n[1], -n[0] * s + n[2] * c];
            }
            out
        })
        .collect()
}

/// Surface-conforming decal shadow quad from the four wheel-contact points:
/// each corner sits `SHADOW_EPS` above the sampled ground along its normal.
pub fn build_shadow_quad(contacts: [[f32; 3]; 4], normals: [[f32; 3]; 4]) -> Quad {
    let mut corners = [[0.0f32; 3]; 4];
    for i in 0..4 {
        for k in 0..3 {
            corners[i][k] =
                contacts[i][k] + normals[i][k] * SHADOW_EPS;
        }
    }
    Quad {
        v: [
            v(corners[0], normals[0]),
            v(corners[1], normals[1]),
            v(corners[2], normals[2]),
            v(corners[3], normals[3]),
        ],
        mat: Material::opaque([0, 0, 0], 1.0, 0.0),
    }
}
