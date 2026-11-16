//! Static prop colliders.
//!
//! The `.SIT` props are scenery, but trees, rocks and buildings need a physical
//! footprint or trucks drive straight through them. A truck only ever touches
//! the part of a prop at body height, so each placement gets vertical cylinders
//! fitted to that slice of the referenced `.SMF`: a tree is its trunk, an arch
//! is its two posts, a wall is a row along its base. offroad4x4 `car.rs`
//! ("scenery collisions (tree trunks, boulders)") collides trees as a 0.4 m
//! trunk; sizing the cylinder from the whole bounding box instead made every
//! canopy, sign arm and start banner a 2.2 m invisible wall on the racing line.
//! Flat plates and tiny debris are skipped so the chassis does not snag on
//! decals.

use std::collections::HashMap;
use std::path::Path;

use macroquad::prelude::*;

use crate::smf::Smf;
use crate::terrain::Track;

#[derive(Clone, Copy, Debug)]
pub struct Collider {
    /// Axis of the cylinder; `y` is the placement's own height.
    pub center: Vec3,
    pub radius: f32,
    pub half_height: f32,
}

/// Widest single cylinder; wider footprints are covered by several.
pub(crate) const MAX_RADIUS: f32 = 2.2;
/// Thinnest cylinder: the trunk radius offroad4x4 uses for every tree.
const MIN_RADIUS: f32 = 0.4;
/// Thinnest fitted cylinder: a sign post or a utility pole, which the retail
/// racing lines pass within about a metre of.
pub(crate) const MIN_POST_RADIUS: f32 = 0.15;
/// Trees are placed by their centre, several metres up; the trunk cylinder
/// reaches this far either side of it so it always spans the ground.
const TREE_HALF_HEIGHT_M: f32 = 30.0;
/// Heights above the ground between which a truck body can touch a prop.
/// Geometry below is a kerb, a low rock or a buried base that the tyres
/// (radius ~0.4 m) roll over; geometry above (canopies, banners, crossbars)
/// is driven under.
const BODY_BAND_LO_M: f32 = 0.6;
const BODY_BAND_HI_M: f32 = 1.8;
/// Footprint samples closer than this to a cylinder's axis join it.
const CLUSTER_LINK_M: f32 = 1.0;
/// Spacing of the samples taken across each triangle.
const SAMPLE_STEP_M: f32 = 0.5;
/// Bound on the sampling grid of one very large triangle.
const MAX_SAMPLES_PER_EDGE: usize = 64;

/// Cylinders (model-space `x`/`z` axis position, radius) covering the part of
/// `smf` between the body-band heights above the ground. `ground` gives the
/// terrain height under a model-space `x`/`z`, itself in model space, so a
/// long prop on a slope is cut along the slope.
pub fn footprint(smf: &Smf, ground: &dyn Fn(f32, f32) -> f32) -> Vec<(Vec2, f32)> {
    let mut points: Vec<Vec2> = Vec::new();
    for m in smf.meshes.iter().filter(|m| m.lod_min <= 0.1) {
        for tri in m.indices.chunks_exact(3) {
            let corner = |i: usize| m.positions.get(tri[i] as usize).copied();
            let (Some(a), Some(b), Some(c)) = (corner(0), corner(1), corner(2)) else {
                continue;
            };
            // Sample the triangle's surface on a barycentric grid and keep
            // what lies inside the band, so a wall is covered between its
            // corners and not only where an edge happens to cross.
            let longest = (b - a).length().max((c - b).length()).max((a - c).length());
            let n = ((longest / SAMPLE_STEP_M).ceil() as usize).clamp(1, MAX_SAMPLES_PER_EDGE);
            for i in 0..=n {
                for j in 0..=(n - i) {
                    let s = a + (b - a) * (i as f32 / n as f32) + (c - a) * (j as f32 / n as f32);
                    let above = s.y - ground(s.x, s.z);
                    if (BODY_BAND_LO_M..=BODY_BAND_HI_M).contains(&above) {
                        points.push(vec2(s.x, s.z));
                    }
                }
            }
        }
    }
    // Greedy clustering: a sample joins the first cylinder whose axis is
    // within reach, moving that axis to the running mean.
    let mut clusters: Vec<(Vec2, f32)> = Vec::new();
    for p in &points {
        match clusters
            .iter_mut()
            .find(|(sum, n)| (*sum / *n - *p).length() < CLUSTER_LINK_M)
        {
            Some((sum, n)) => {
                *sum += *p;
                *n += 1.0;
            }
            None => clusters.push((*p, 1.0)),
        }
    }
    let axes: Vec<Vec2> = clusters.iter().map(|(sum, n)| *sum / *n).collect();
    let mut radii = vec![0.0f32; axes.len()];
    for p in &points {
        let mut best = (f32::MAX, 0usize);
        for (i, a) in axes.iter().enumerate() {
            let d = (*a - *p).length();
            if d < best.0 {
                best = (d, i);
            }
        }
        if let Some(r) = radii.get_mut(best.1) {
            *r = r.max(best.0);
        }
    }
    axes.into_iter()
        .zip(radii)
        .map(|(a, r)| (a, r.clamp(MIN_POST_RADIUS, MAX_RADIUS)))
        .collect()
}

/// Build the static collider set for a track. Parsed models are cached per
/// filename, so dense `.SIT` files only read each mesh once; the footprint is
/// cut per placement because it depends on how deep the prop sits in the
/// ground.
pub fn build(track: &Track) -> Vec<Collider> {
    let mut cache: HashMap<String, Option<(Smf, f32)>> = HashMap::new();
    let mut out = Vec::new();
    for prop in &track.props {
        // Moving traffic (`!type,flags` 10 with a `bvel`) starts in the middle
        // of the road. Until traffic drives, a solid taxi frozen on the racing
        // line is a wall the original never had, so it gets no collider (and
        // is not drawn either).
        if prop.is_unported_traffic() {
            continue;
        }
        // A tree is its trunk (offroad4x4 `car.rs`: `reach = body_r + 0.4`),
        // whatever its lowest branches do; the trunk is the mesh origin.
        if prop.kind == crate::terrain::PROP_KIND_TREE {
            out.push(Collider {
                center: prop.pos,
                radius: MIN_RADIUS,
                half_height: TREE_HALF_HEIGHT_M,
            });
            continue;
        }
        let entry = cache.entry(prop.model.clone()).or_insert_with(|| {
            let path = Path::new(crate::render::MODEL_DIR).join(format!("{}.SMF", prop.model));
            let text = std::fs::read_to_string(path).ok()?;
            let smf = Smf::parse(&text).ok()?;
            let (lo, hi) = smf.bounds();
            let size = hi - lo;
            if size.x.max(size.z) * 0.5 < 0.25 || size.y < 0.6 {
                return None;
            }
            Some((smf, size.y))
        });
        let Some((smf, height)) = entry else {
            continue;
        };
        // Same model-to-world turn as `Renderer::draw_model`.
        let basis = Quat::from_rotation_y(prop.yaw) * Quat::from_rotation_y(std::f32::consts::PI);
        // The mesh origin is the placement position, so terrain heights are
        // brought into model space by subtracting it.
        let ground = |x: f32, z: f32| {
            let w = prop.pos + basis * vec3(x, 0.0, z);
            track.height_at_world(w.x, w.z) - prop.pos.y
        };
        let cylinders = footprint(smf, &ground);
        for (axis, radius) in cylinders.iter() {
            out.push(Collider {
                center: prop.pos + basis * vec3(axis.x, 0.0, axis.y),
                radius: *radius,
                half_height: *height * 0.5,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{Prop, GRID};

    fn track_with(props: Vec<Prop>) -> Track {
        Track {
            id: "c".into(),
            dir: std::path::PathBuf::new(),
            heights: vec![0; GRID * GRID],
            colors: vec![0; GRID * GRID],
            shadows: vec![0; GRID * GRID],
            scale: 1.0,
            height_offset: 0.0,
            water_height: 0.0,
            water_color: Color::new(0.20, 0.44, 0.60, 0.72),
            fog_color: Color::new(0.56, 0.71, 0.86, 1.0),
            textures: vec![],
            detail_texture: None,
            sky_texture: None,
            light_dir: Vec3::Y,
            props,
            spawns: vec![],
            gates: vec![],
            paths: vec![],
            tints: vec![],
            ground_colors: vec![],
            atlas: None,
        }
    }

    /// A box from `lo` to `hi` as one LOD-0 sub-mesh.
    fn block(lo: Vec3, hi: Vec3) -> crate::smf::SmfMesh {
        let mut positions = Vec::new();
        for y in [lo.y, hi.y] {
            for (x, z) in [(lo.x, lo.z), (hi.x, lo.z), (hi.x, hi.z), (lo.x, hi.z)] {
                positions.push(vec3(x, y, z));
            }
        }
        let mut indices = Vec::new();
        for i in 0..4u16 {
            let j = (i + 1) % 4;
            indices.extend_from_slice(&[i, j, i + 4, j, j + 4, i + 4]);
        }
        indices.extend_from_slice(&[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        crate::smf::SmfMesh {
            name: "b".into(),
            opaque: true,
            lod_min: 0.0,
            texture: String::new(),
            positions,
            normals: vec![],
            uvs: vec![],
            indices,
            radius: 0.0,
        }
    }

    /// A tree is its trunk: the 3 m canopy above body height used to make
    /// the whole thing a 1.5 m solid cylinder.
    #[test]
    fn tree_collides_as_its_trunk_not_its_canopy() {
        let smf = Smf {
            meshes: vec![
                block(vec3(-0.2, 0.0, -0.2), vec3(0.2, 4.0, 0.2)),
                block(vec3(-1.5, 2.5, -1.5), vec3(1.5, 5.0, 1.5)),
            ],
        };
        let f = footprint(&smf, &|_, _| 0.0);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].0.length() < 0.1);
        assert!(f[0].1 < 0.4, "radius {}", f[0].1);
    }

    /// A start arch is two posts; the road between them must stay open.
    #[test]
    fn arch_leaves_the_road_between_its_posts_open() {
        let smf = Smf {
            meshes: vec![
                block(vec3(-5.2, 0.0, -0.2), vec3(-4.8, 5.0, 0.2)),
                block(vec3(4.8, 0.0, -0.2), vec3(5.2, 5.0, 0.2)),
                block(vec3(-5.2, 4.5, -0.2), vec3(5.2, 5.0, 0.2)),
            ],
        };
        let f = footprint(&smf, &|_, _| 0.0);
        assert_eq!(f.len(), 2, "{f:?}");
        for (axis, radius) in &f {
            assert!((axis.x.abs() - 5.0).abs() < 0.2, "post at {axis:?}");
            assert!(axis.x.abs() - radius > 3.0, "post blocks the road");
        }
    }

    /// A wall is covered along its whole base, not just at its centre.
    #[test]
    fn wall_is_solid_along_its_length() {
        let smf = Smf {
            meshes: vec![block(vec3(-6.0, 0.0, -0.2), vec3(6.0, 3.0, 0.2))],
        };
        let f = footprint(&smf, &|_, _| 0.0);
        for x in [-5.5f32, -3.0, 0.0, 3.0, 5.5] {
            assert!(
                f.iter().any(|(a, r)| (*a - vec2(x, 0.0)).length() < *r),
                "gap in the wall at x={x}: {f:?}"
            );
        }
    }

    #[test]
    fn collider_radius_is_bounded() {
        let t = track_with(vec![Prop {
            pos: vec3(1.0, 2.0, 3.0),
            yaw: 0.0,
            model: "AB3CRA".into(),
            ..Default::default()
        }]);
        if !Path::new(crate::render::MODEL_DIR)
            .join("AB3CRA.SMF")
            .exists()
        {
            return;
        }
        let c = build(&t);
        assert!(!c.is_empty());
        for c in &c {
            assert!(c.radius >= MIN_POST_RADIUS && c.radius <= MAX_RADIUS);
            assert!(c.half_height > 0.0);
            assert_eq!(c.center.y, 2.0);
        }
    }

    /// THEHILL's taxi and lorries start on the racing line with a `bvel`;
    /// parked there as solid scenery they stopped every truck dead.
    #[test]
    fn moving_traffic_gets_no_static_collider() {
        if !Path::new(crate::render::MODEL_DIR)
            .join("AB3CRA.SMF")
            .exists()
        {
            return;
        }
        let prop = |kind: u8, moving: bool| Prop {
            pos: vec3(1.0, 2.0, 3.0),
            model: "AB3CRA".into(),
            kind,
            moving,
            ..Default::default()
        };
        assert!(!build(&track_with(vec![prop(7, false)])).is_empty());
        assert!(build(&track_with(vec![prop(
            crate::terrain::PROP_KIND_TRAFFIC,
            true
        )]))
        .is_empty());
    }

    /// Type 12 boxes are trees: one trunk-sized cylinder on the placement,
    /// without needing the mesh at all.
    #[test]
    fn tree_boxes_are_a_single_trunk() {
        let c = build(&track_with(vec![Prop {
            pos: vec3(4.0, 9.0, 6.0),
            model: "ANY_PINE".into(),
            kind: crate::terrain::PROP_KIND_TREE,
            ..Default::default()
        }]));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].center, vec3(4.0, 9.0, 6.0));
        assert_eq!(c[0].radius, MIN_RADIUS);
    }

    #[test]
    fn unknown_models_are_skipped() {
        let t = track_with(vec![Prop {
            pos: Vec3::ZERO,
            yaw: 0.0,
            model: "NOT_A_MODEL".into(),
            ..Default::default()
        }]);
        assert!(build(&t).is_empty());
    }
}
