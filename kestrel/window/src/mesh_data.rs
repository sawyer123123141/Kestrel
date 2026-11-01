//! What `draw3d.make_mesh` does to a `MeshData` before raylib sees it, with no raylib in it:
//! checking it, splitting a big indexed mesh into parts raylib's 16-bit indices can address, and
//! converting its colours to bytes.

use crate::abi::KColor;

/// The most vertices one part of a mesh may have: raylib's indices are `unsigned short`.
pub const MOST_PART_VERTICES: usize = 1 << 16;

/// Checks `MeshData`'s lengths and indices; the error is the spec's message, naming draw3d.make_mesh.
///
/// In order: one colour per vertex or none, then one uv per vertex or none, then three indices a
/// triangle (or, with no indices, three vertices), then the sizes raylib can take (`limits`), then
/// each index, the first one out of range being the one named.
pub fn check(vertices: usize, colors: usize, uvs: usize, indices: &[i32]) -> Result<(), String> {
    for (count, what) in [(colors, "color"), (uvs, "uv")] {
        if count != 0 && count != vertices {
            return Err(format!("draw3d.make_mesh needs one {what} per vertex or none: {vertices} vertices, {count} {what}s"));
        }
    }
    if indices.is_empty() {
        if !vertices.is_multiple_of(3) {
            return Err(format!("draw3d.make_mesh needs a multiple of 3 vertices when there are no indices, not {vertices}"));
        }
    } else if !indices.len().is_multiple_of(3) {
        return Err(format!("draw3d.make_mesh needs a multiple of 3 indices, not {}", indices.len()));
    }
    limits(vertices, indices.len())?;
    match indices.iter().find(|&&i| usize::try_from(i).map_or(true, |i| i >= vertices)) {
        Some(i) => Err(format!("draw3d.make_mesh got index {i}, but there are only {vertices} vertices")),
        None => Ok(()),
    }
}

/// Checks that raylib can take a mesh of `vertices` vertices and `indices` indices. raylib gives
/// each buffer's size in bytes as a C `int`:
///
/// - a mesh without indices is one part, whose biggest buffer, the positions, takes 12 bytes a vertex;
/// - an indexed mesh's parts have at most `MOST_PART_VERTICES` vertices, but a part's indices take
///   2 bytes each, and a part can have all of them;
/// - and the whole mesh's vertex count is the `Mesh`'s `vertex_count`, an `int`. (Its triangle
///   count is at most a third of either count, so it fits too.)
pub fn limits(vertices: usize, indices: usize) -> Result<(), String> {
    let int = i32::MAX as usize;
    let too_many = |n: usize, what: &str, mesh: &str, most: usize| {
        Err(format!("draw3d.make_mesh got {n} {what}, but {mesh} can have at most {most}; split it into smaller meshes"))
    };
    if indices == 0 && vertices > int / 12 {
        return too_many(vertices, "vertices", "a mesh without indices", int / 12);
    }
    if vertices > int {
        return too_many(vertices, "vertices", "a mesh", int);
    }
    if indices > int / 2 {
        return too_many(indices, "indices", "a mesh", int / 2);
    }
    Ok(())
}

/// One part of a split mesh.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    /// The original vertex numbers the part uses, in the order it first uses them.
    pub vertices: Vec<u32>,
    /// The part's triangles, three numbers each, as positions in `vertices`.
    pub indices: Vec<u16>,
}

/// Groups an indexed mesh's triangles, in order, into parts of at most `max` vertices each
/// (65,536 for raylib's 16-bit indices). Each part lists the original vertex numbers it uses and
/// its own `u16` indices into them. An unindexed mesh needs no split.
///
/// A triangle goes in the current part if the vertices it adds still fit, or else starts a new
/// part. A vertex that triangles in two parts use is in both, so the parts draw exactly the
/// original triangles, in order.
///
/// `indices` must have passed `check`: three a triangle, none negative. `max` must be from 3
/// (a triangle always fits an empty part) to `MOST_PART_VERTICES` (a part's numbers fit a `u16`).
pub fn split(indices: &[i32], max: usize) -> Vec<Part> {
    assert!((3..=MOST_PART_VERTICES).contains(&max), "a part of {max} vertices");
    let vertex = |i: i32| usize::try_from(i).expect("`check` refuses negative indices");
    let count = indices.iter().map(|&i| vertex(i) + 1).max().unwrap_or(0);
    // For each original vertex: the last part that uses it (`NONE` for none yet), and its number
    // in that part. There are at most a third as many parts as indices, which `check` keeps below
    // 2^30, so a part's number fits a `u32` and is never `NONE`.
    const NONE: u32 = u32::MAX;
    let mut seen = vec![(NONE, 0u16); count];
    let mut parts: Vec<Part> = Vec::new();
    for triangle in indices.as_chunks::<3>().0 {
        // Whether the vertices the triangle adds to the current part (each one once) still fit.
        let fits = parts.last().is_some_and(|part| {
            let current = (parts.len() - 1) as u32;
            let fresh = |k: usize| seen[vertex(triangle[k])].0 != current && !triangle[..k].contains(&triangle[k]);
            part.vertices.len() + (0..3).filter(|&k| fresh(k)).count() <= max
        });
        if !fits {
            parts.push(Part::default());
        }
        let number = (parts.len() - 1) as u32;
        let part = parts.last_mut().expect("there is a part now");
        for &i in triangle {
            let slot = &mut seen[vertex(i)];
            if slot.0 != number {
                // Below `max`, which is at most 65,536, so the number fits a `u16`.
                *slot = (number, part.vertices.len() as u16);
                part.vertices.push(i as u32);
            }
            part.indices.push(slot.1);
        }
    }
    parts
}

/// Colours as raylib's RGBA bytes.
pub fn colour_bytes(colors: &[KColor]) -> Vec<u8> {
    colors.iter().flat_map(|&c| rgba(c)).collect()
}

/// One colour as raylib's four bytes, clamped and rounded like `KColor::to_raylib`.
pub fn rgba(c: KColor) -> [u8; 4] {
    let c = c.to_raylib();
    [c.r, c.g, c.b, c.a]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(vertices: usize, colors: usize, uvs: usize, indices: &[i32]) -> String {
        check(vertices, colors, uvs, indices).unwrap_err()
    }

    #[test]
    fn good_meshes_pass() {
        assert_eq!(check(4, 4, 0, &[0, 1, 2, 0, 2, 3]), Ok(()), "a quad with colours");
        assert_eq!(check(4, 0, 4, &[0, 1, 2, 0, 2, 3]), Ok(()), "with uvs and no colours");
        assert_eq!(check(6, 6, 6, &[]), Ok(()), "unindexed: two triangles of three vertices");
        assert_eq!(check(0, 0, 0, &[]), Ok(()), "empty");
        assert_eq!(check(3, 0, 0, &[2, 2, 2]), Ok(()), "a vertex used by every corner");
        assert_eq!(check(5, 0, 0, &[0, 1, 4]), Ok(()), "vertices no triangle uses");
    }

    #[test]
    fn colours_and_uvs_are_one_per_vertex_or_none() {
        assert_eq!(error(24, 23, 0, &[]), "draw3d.make_mesh needs one color per vertex or none: 24 vertices, 23 colors");
        assert_eq!(error(3, 4, 0, &[]), "draw3d.make_mesh needs one color per vertex or none: 3 vertices, 4 colors");
        assert_eq!(error(24, 0, 25, &[]), "draw3d.make_mesh needs one uv per vertex or none: 24 vertices, 25 uvs");
        assert_eq!(error(0, 1, 0, &[]), "draw3d.make_mesh needs one color per vertex or none: 0 vertices, 1 colors");
        // Colours are checked before uvs.
        assert_eq!(error(3, 1, 1, &[]), "draw3d.make_mesh needs one color per vertex or none: 3 vertices, 1 colors");
    }

    #[test]
    fn indices_come_in_threes_and_name_vertices_that_exist() {
        let seven = [0, 1, 2, 0, 2, 3, 0];
        assert_eq!(error(24, 0, 0, &seven), "draw3d.make_mesh needs a multiple of 3 indices, not 7");
        assert_eq!(error(24, 0, 0, &[0, 1, 30]), "draw3d.make_mesh got index 30, but there are only 24 vertices");
        assert_eq!(error(24, 0, 0, &[0, 1, 24]), "draw3d.make_mesh got index 24, but there are only 24 vertices");
        assert_eq!(error(24, 0, 0, &[0, -1, 2]), "draw3d.make_mesh got index -1, but there are only 24 vertices");
        assert_eq!(error(0, 0, 0, &[0, 0, 0]), "draw3d.make_mesh got index 0, but there are only 0 vertices");
        // The first wrong index is the one named.
        assert_eq!(error(4, 0, 0, &[0, 9, -2]), "draw3d.make_mesh got index 9, but there are only 4 vertices");
    }

    #[test]
    fn with_no_indices_the_vertices_come_in_threes() {
        assert_eq!(error(25, 0, 0, &[]), "draw3d.make_mesh needs a multiple of 3 vertices when there are no indices, not 25");
        assert_eq!(error(1, 1, 1, &[]), "draw3d.make_mesh needs a multiple of 3 vertices when there are no indices, not 1");
        // With indices, any number of vertices will do.
        assert_eq!(check(25, 0, 0, &[0, 1, 2]), Ok(()));
    }

    #[test]
    fn sizes_raylib_cannot_take_are_refused() {
        // An unindexed mesh is one raylib buffer of 12 bytes a vertex, whose size is a C `int`.
        let most = i32::MAX as usize / 12;
        assert_eq!(limits(most, 0), Ok(()));
        assert_eq!(
            limits(most + 1, 0),
            Err(format!(
                "draw3d.make_mesh got {} vertices, but a mesh without indices can have at most {most}; split it into smaller meshes",
                most + 1
            ))
        );
        // An indexed mesh's parts have at most 65,536 vertices, but each part's indices are one
        // buffer of 2 bytes an index, and the whole mesh's vertex count must fit an `int`.
        let most_indices = i32::MAX as usize / 2;
        assert_eq!(limits(i32::MAX as usize, most_indices), Ok(()));
        assert_eq!(
            limits(4, most_indices + 1),
            Err(format!(
                "draw3d.make_mesh got {} indices, but a mesh can have at most {most_indices}; split it into smaller meshes",
                most_indices + 1
            ))
        );
        assert_eq!(
            limits(i32::MAX as usize + 1, 3),
            Err(format!(
                "draw3d.make_mesh got {} vertices, but a mesh can have at most {}; split it into smaller meshes",
                i32::MAX as usize + 1,
                i32::MAX
            ))
        );
    }

    /// `parts`' triangles as original vertex numbers, in order.
    fn triangles_of(parts: &[Part]) -> Vec<u32> {
        parts.iter().flat_map(|p| p.indices.iter().map(|&i| p.vertices[usize::from(i)])).collect()
    }

    /// The indices of `quads` quads, four vertices each, as `MeshData.quad` adds them.
    fn quads(quads: u32) -> Vec<i32> {
        (0..quads as i32).flat_map(|q| [0, 1, 2, 0, 2, 3].map(|k| 4 * q + k)).collect()
    }

    #[test]
    fn a_70000_vertex_mesh_splits_into_two_parts_that_draw_the_same_triangles() {
        let indices = quads(17_500);
        let parts = split(&indices, MOST_PART_VERTICES);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| p.vertices.len() <= MOST_PART_VERTICES));
        // The first part is full: 16,384 quads. The rest go in the second.
        assert_eq!(parts.iter().map(|p| p.vertices.len()).collect::<Vec<_>>(), [65_536, 4_464]);
        assert_eq!(parts.iter().map(|p| p.indices.len()).collect::<Vec<_>>(), [6 * 16_384, 6 * 1_116]);
        let original: Vec<u32> = indices.iter().map(|&i| i as u32).collect();
        assert_eq!(triangles_of(&parts), original);
        // Each part numbers its vertices from 0.
        assert_eq!(parts[1].vertices[0], 65_536);
        assert_eq!(parts[1].indices[..6], [0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn a_vertex_shared_across_parts_is_in_each_part_that_uses_it() {
        // A fan: every triangle uses vertex 0. A part of 5 vertices holds three of its triangles:
        // the first brings three vertices, and each after it one more.
        let fan: Vec<i32> = (1..=9).flat_map(|k| [0, k, k + 1]).collect();
        let parts = split(&fan, 5);
        assert!(parts.iter().all(|p| p.vertices.len() <= 5 && p.vertices[0] == 0), "{parts:?}");
        assert_eq!(parts.iter().map(|p| p.indices.len() / 3).collect::<Vec<_>>(), [3, 3, 3]);
        let original: Vec<u32> = fan.iter().map(|&i| i as u32).collect();
        assert_eq!(triangles_of(&parts), original);
    }

    #[test]
    fn a_triangle_that_repeats_a_vertex_counts_it_once() {
        // Two triangles of three vertices fill a part of 6; [6, 6, 7] adds two more, so it starts a new part,
        // and [7, 7, 7] fits beside it.
        let indices = [0, 1, 2, 3, 4, 5, 6, 6, 7, 7, 7, 7];
        let parts = split(&indices, 6);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], Part { vertices: vec![6, 7], indices: vec![0, 0, 1, 1, 1, 1] });
        assert_eq!(triangles_of(&parts), indices.map(|i| i as u32));
        // A part of 3 can still take a degenerate triangle beside two new vertices.
        assert_eq!(split(&[0, 1, 1, 2, 2, 2], 3).len(), 1);
    }

    #[test]
    fn a_mesh_that_fits_is_one_part_and_no_indices_is_no_parts() {
        let indices = quads(3);
        let parts = split(&indices, MOST_PART_VERTICES);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].vertices, (0..12).collect::<Vec<u32>>());
        assert!(split(&[], MOST_PART_VERTICES).is_empty());
    }

    #[test]
    fn colour_bytes_clamp_and_round_like_to_raylib() {
        let colors = [
            KColor { r: 1.0, g: 0.3, b: -1.0, a: 2.0 },
            KColor { r: 0.5, g: 0.0, b: 1.0, a: 0.0 },
            KColor { r: f32::NAN, g: 0.002, b: 0.998, a: 1.0 },
        ];
        let bytes = colour_bytes(&colors);
        let expected: Vec<u8> = colors
            .iter()
            .flat_map(|c| {
                let c = c.to_raylib();
                [c.r, c.g, c.b, c.a]
            })
            .collect();
        assert_eq!(bytes, expected);
        assert_eq!(bytes[..8], [255, 77, 0, 255, 128, 0, 255, 0]);
        assert!(colour_bytes(&[]).is_empty());
    }
}
