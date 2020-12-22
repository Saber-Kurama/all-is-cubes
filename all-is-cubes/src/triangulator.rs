// Copyright 2020 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <http://opensource.org/licenses/MIT>.

//! Algorithms for converting blocks/voxels to triangle-based rendering
//! (as opposed to raytracing, voxel display hardware, or whatever else).
//!
//! All of the algorithms here are independent of graphics API but may presume that
//! one exists and has specific data types to specialize in.
//!
//! Note on terminology: Some sources say that “tesselation” would be a better name
//! for this operation than “triangulation”. However, “tesselation” means a specific
//! other operation in OpenGL graphics programming, and “triangulation” seems to
//! be the more commonly used terms.

use cgmath::{EuclideanSpace as _, Point3, Transform as _, Vector2, Vector3};
use std::convert::TryFrom;

use crate::block::{EvaluatedBlock, Resolution};
use crate::math::{Face, FaceMap, FreeCoordinate, GridCoordinate, RGBA};
use crate::space::{Grid, PackedLight, Space};
use crate::util::ConciseDebug as _;

/// Numeric type used to store texture coordinates.
pub type TextureCoordinate = f32;

/// Generic structure of output from triangulator. Implement
/// <code>[`From`]&lt;[`BlockVertex`]&gt;</code>
/// to provide a specialized version fit for the target graphics API.
#[derive(Clone, Copy, PartialEq)]
pub struct BlockVertex {
    /// Vertex position.
    pub position: Point3<FreeCoordinate>,
    /// Vertex normal.
    ///
    /// This is always an axis-aligned unit vector when generated by [`triangulate_blocks`]
    /// and [`triangulate_space`].
    pub normal: Vector3<FreeCoordinate>, // TODO: Use a smaller number type? Storage vs convenience?
    /// Surface color or texture coordinate.
    pub coloring: Coloring,
}
/// Describes the two ways a [`BlockVertex`] may be colored; by a solid color or by a texture.
#[derive(Clone, Copy, PartialEq)]
pub enum Coloring {
    /// Solid color.
    Solid(RGBA),
    /// Texture coordinates provided by the [`TextureAllocator`] for this vertex.
    Texture(Vector3<TextureCoordinate>),
}

impl std::fmt::Debug for BlockVertex {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        // Print compactly on single line even if the formatter is in prettyprint mode.
        write!(
            fmt,
            "{{ p: {:?} n: {:?} c: {:?} }}",
            self.position.as_concise_debug(),
            self.normal.cast::<i8>().unwrap().as_concise_debug(), // no decimals!
            self.coloring
        )
    }
}
impl std::fmt::Debug for Coloring {
    // TODO: test formatting of this
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Coloring::Solid(color) => write!(fmt, "Solid({:?})", color),
            Coloring::Texture(tc) => write!(fmt, "Texture({:?})", tc.as_concise_debug()),
        }
    }
}

/// Implement this trait along with <code>[`From`]&lt;[`BlockVertex`]&gt;</code> to
/// provide a representation of [`BlockVertex`] suitable for the target graphics system.
pub trait ToGfxVertex<GV>: From<BlockVertex> + Sized {
    /// Number type for the vertex position coordinates.
    type Coordinate: cgmath::BaseNum;

    /// Transforms a vertex of a general model of an [`EvaluatedBlock`] to its
    /// instantiation in a specific location in space and lighting conditions.
    fn instantiate(&self, offset: Vector3<Self::Coordinate>, lighting: PackedLight) -> GV;
}

/// Trivial implementation of [`ToGfxVertex`] for testing purposes. Discards lighting.
impl ToGfxVertex<BlockVertex> for BlockVertex {
    type Coordinate = FreeCoordinate;
    fn instantiate(&self, offset: Vector3<FreeCoordinate>, _lighting: PackedLight) -> Self {
        Self {
            position: self.position + offset,
            ..*self
        }
    }
}

/// Describes how to draw one [`Face`] of a [`Block`].
///
/// See [`BlockTriangulation`] for a description of how triangles are grouped into faces.
/// The texture associated with the contained vertices' texture coordinates is also
/// kept there.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FaceTriangulation<V> {
    /// Vertices of triangles (i.e. length is a multiple of 3) in counterclockwise order.
    vertices: Vec<V>,
    /// Whether the block entirely fills its cube, such that nothing can be seen through
    /// it and faces of adjacent blocks may be removed.
    fully_opaque: bool,
}

impl<V> Default for FaceTriangulation<V> {
    fn default() -> Self {
        FaceTriangulation {
            vertices: Vec::new(),
            fully_opaque: false,
        }
    }
}

/// Describes how to draw a block. Pass it to [`triangulate_space`] to use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockTriangulation<V, T> {
    /// Vertices grouped by the face they belong to.
    ///
    /// All triangles which are on the surface of the cube (such that they may be omitted
    /// when a `fully_opaque` block is adjacent) are grouped under the corresponding
    /// face, and all other triangles are grouped under `Face::WITHIN`.
    faces: FaceMap<FaceTriangulation<V>>,

    /// Texture tiles used by the vertices; holding these objects is intended to ensure
    /// the texture coordinates stay valid.
    textures_used: Vec<T>,
}

impl<V, T> BlockTriangulation<V, T> {
    /// Return the textures used for this block. This may be used to retain the textures
    /// for as long as the associated vertices are being used, rather than only as long as
    /// the life of this triangulation.
    // TODO: revisit this interface design. Maybe callers should just have an Rc<BlockTriangulation>?
    pub(crate) fn textures(&self) -> &[T] {
        &self.textures_used
    }
}

impl<V, T> Default for BlockTriangulation<V, T> {
    fn default() -> Self {
        Self {
            faces: FaceMap::generate(|_| FaceTriangulation::default()),
            textures_used: Vec::new(),
        }
    }
}

/// Array of [`BlockTriangulation`] indexed by a [`Space`]'s block indices; a convenience
/// alias for the return type of [`triangulate_blocks`].
/// Pass it to [`triangulate_space`] to use it.
pub type BlockTriangulations<V, A> = Box<[BlockTriangulation<V, A>]>;

const QUAD_VERTICES: &[Point3<FreeCoordinate>; 6] = &[
    // Two-triangle quad.
    // Note that looked at from a X-right Y-up view, these triangles are
    // clockwise, but they're properly counterclockwise from the perspective
    // that we're drawing the face _facing towards negative Z_ (into the screen),
    // which is how cube faces as implicitly defined by Face::matrix work.
    Point3::new(0.0, 0.0, 0.0),
    Point3::new(0.0, 1.0, 0.0),
    Point3::new(1.0, 0.0, 0.0),
    Point3::new(1.0, 0.0, 0.0),
    Point3::new(0.0, 1.0, 0.0),
    Point3::new(1.0, 1.0, 0.0),
];

#[inline]
fn push_quad_solid<V: From<BlockVertex>>(vertices: &mut Vec<V>, face: Face, color: RGBA) {
    let transform = face.matrix();
    for &p in QUAD_VERTICES {
        vertices.push(V::from(BlockVertex {
            position: transform.transform_point(p),
            normal: face.normal_vector(),
            coloring: Coloring::Solid(color),
        }));
    }
}

#[inline]
fn push_quad_textured<V: From<BlockVertex>>(
    vertices: &mut Vec<V>,
    face: Face,
    depth: FreeCoordinate,
    texture_tile: &impl TextureTile,
) {
    let transform = face.matrix();
    for &p in QUAD_VERTICES {
        vertices.push(V::from(BlockVertex {
            position: transform.transform_point(p + Vector3::new(0.0, 0.0, depth)),
            normal: face.normal_vector(),
            coloring: Coloring::Texture(texture_tile.texcoord(Vector2::new(
                p.x as TextureCoordinate,
                p.y as TextureCoordinate,
            ))),
        }));
    }
}

/// Generate [`BlockTriangulation`] for a block.
fn triangulate_block<V: From<BlockVertex>, A: TextureAllocator>(
    // TODO: Arrange to pass in a buffer of old data such that we can reuse existing textures.
    // This will allow for efficient implementation of animated blocks.
    block: &EvaluatedBlock,
    texture_allocator: &mut A,
) -> BlockTriangulation<V, A::Tile> {
    match &block.voxels {
        None => {
            let faces = FaceMap::generate(|face| {
                if face == Face::WITHIN {
                    // No interior detail for atom blocks.
                    return FaceTriangulation::default();
                }

                let fully_opaque = block.color.fully_opaque();
                FaceTriangulation {
                    // TODO: Port over pseudo-transparency mechanism, then change this to a
                    // within-epsilon-of-zero test. ...conditional on `GfxVertex` specifying support.
                    vertices: if fully_opaque {
                        let mut face_vertices: Vec<V> = Vec::with_capacity(6);
                        push_quad_solid(&mut face_vertices, face, block.color);
                        face_vertices
                    } else {
                        Vec::new()
                    },
                    fully_opaque,
                }
            });

            BlockTriangulation {
                faces,
                textures_used: vec![],
            }
        }
        Some(voxels) => {
            // Construct empty output to mutate, because inside the loops we'll be
            // updating WITHIN independently of other faces.
            let mut output_by_face = FaceMap::generate(|face| FaceTriangulation {
                vertices: Vec::new(),
                // Start assuming opacity; if we find any transparent pixels we'll set
                // this to false. WITHIN is always "transparent" because the algorithm
                // that consumes this structure will say "draw this face if its adjacent
                // cube's opposing face is not opaque", and WITHIN means the adjacent
                // cube is ourself.
                fully_opaque: face != Face::WITHIN,
            });
            let mut textures_used = Vec::new();

            // Use the size from the textures, regardless of what the actual tile size is,
            // because this won't panic and the other strategy will. TODO: Implement
            // dynamic choice of texture size.
            let resolution: GridCoordinate = texture_allocator.resolution();

            let out_of_bounds_color = RGBA::new(1.0, 1.0, 0.0, 1.0);

            for &face in Face::ALL_SIX {
                let transform = face.matrix();

                // Layer 0 is the outside surface of the cube and successive layers are
                // deeper inside.
                for layer in 0..resolution {
                    // TODO: JS version would detect fully-opaque blocks (a derived property of Block)
                    // and only scan the first and last faces
                    let mut tile_texels: Vec<(u8, u8, u8, u8)> =
                        Vec::with_capacity((resolution as usize).pow(2));
                    let mut layer_is_visible_somewhere = false;
                    for t in 0..resolution {
                        for s in 0..resolution {
                            // TODO: Matrix4 isn't allowed to be integer. Make Face provide a better strategy.
                            // While we're at it, also implement the optimization that positive and negative
                            // faces can share a texture sometimes (which requires dropping the property
                            // Face::matrix provides where all transforms contain no mirroring).
                            let cube: Point3<GridCoordinate> = (transform.transform_point(
                                (Point3::new(
                                    FreeCoordinate::from(s),
                                    FreeCoordinate::from(t),
                                    FreeCoordinate::from(layer),
                                ) + Vector3::new(0.5, 0.5, 0.5))
                                    / FreeCoordinate::from(resolution),
                            ) * FreeCoordinate::from(
                                resolution,
                            ) - Vector3::new(0.5, 0.5, 0.5))
                            .cast::<GridCoordinate>()
                            .unwrap();

                            // Diagnose out-of-space accesses. TODO: Tidy this up and document it, or remove it:
                            // it will happen whenever the space is the wrong size for the textures.
                            let color = voxels.get(cube).unwrap_or(&out_of_bounds_color);

                            if !color.fully_transparent() && {
                                // Compute whether this voxel is not hidden behind another
                                let obscuring_cube = cube + face.normal_vector();
                                !voxels
                                    .get(obscuring_cube)
                                    .map(|c| c.fully_opaque())
                                    .unwrap_or(false)
                            } {
                                layer_is_visible_somewhere = true;
                            }

                            if layer == 0 && !color.fully_opaque() {
                                // If the first layer is transparent somewhere...
                                output_by_face[face].fully_opaque = false;
                            }

                            tile_texels.push(color.to_saturating_32bit());
                        }
                    }
                    if layer_is_visible_somewhere {
                        // Actually store and use the texels we just computed.
                        let mut texture_tile = texture_allocator.allocate();
                        texture_tile.write(tile_texels.as_ref());
                        push_quad_textured(
                            // Only the surface faces go anywhere but WITHIN.
                            &mut output_by_face[if layer == 0 { face } else { Face::WITHIN }]
                                .vertices,
                            face,
                            FreeCoordinate::from(layer) / FreeCoordinate::from(resolution),
                            &texture_tile,
                        );
                        textures_used.push(texture_tile);
                    }
                }
            }

            BlockTriangulation {
                faces: output_by_face,
                textures_used,
            }
        }
    }
}

/// Precomputes vertices for blocks present in a space.
///
/// The resulting array is indexed by the `Space`'s internal unstable IDs.
pub fn triangulate_blocks<V: From<BlockVertex>, A: TextureAllocator>(
    space: &Space,
    texture_allocator: &mut A,
) -> BlockTriangulations<V, A::Tile> {
    space
        .block_data()
        .iter()
        .map(|block_data| triangulate_block(block_data.evaluated(), texture_allocator))
        .collect()
}

/// Allocate an empty output buffer for [`triangulate_space`] to write into.
// TODO: Make this a struct's `Default`.
pub fn new_space_buffer<V>() -> FaceMap<Vec<V>> {
    FaceMap::generate(|_| Vec::new())
}

/// Computes a triangle-based representation of a [`Space`] for rasterization.
///
/// `block_triangulations` should be the result of [`triangulate_blocks`] or equivalent,
/// and must be up-to-date with the [`Space`]'s blocks or the result will be inaccurate
/// and may contain severe lighting errors.
///
/// The triangles will be written into `output_vertices`, replacing the existing
/// contents. This is intended to avoid memory reallocation in the common case of
/// new geometry being similar to old geometry.
///
/// `output_vertices` is a [`FaceMap`] dividing the faces according to their normal
/// vectors.
///
/// Note about edge case behavior: This algorithm does not use the [`Space`]'s block data
/// at all. Thus, it always has a consistent interpretation based on
/// `block_triangulations` (as opposed to, for example, using face opacity data not the
/// same as the meshes and thus producing a rendering with gaps in it)..
pub fn triangulate_space<BV, GV, T>(
    space: &Space,
    bounds: Grid,
    block_triangulations: &[BlockTriangulation<BV, T>],
    output_vertices: &mut FaceMap<Vec<GV>>,
) where
    BV: ToGfxVertex<GV>,
{
    let empty_render = BlockTriangulation::<BV, T>::default();
    let lookup = |cube| {
        match space.get_block_index(cube) {
            // TODO: On out-of-range, draw an obviously invalid block instead of an invisible one.
            Some(index) => &block_triangulations
                .get(index as usize)
                .unwrap_or(&empty_render),
            None => &empty_render,
        }
    };

    for &face in Face::ALL_SEVEN.iter() {
        // use the buffer but not the existing data
        output_vertices[face].clear();
    }
    for cube in bounds.interior_iter() {
        let precomputed = lookup(cube);
        let low_corner = cube.cast::<BV::Coordinate>().unwrap();
        for &face in Face::ALL_SEVEN {
            let adjacent_cube = cube + face.normal_vector();
            if lookup(adjacent_cube).faces[face.opposite()].fully_opaque {
                // Don't draw obscured faces
                continue;
            }

            let lighting = space.get_lighting(adjacent_cube);

            // Copy vertices, offset to the block position and with lighting
            for vertex in precomputed.faces[face].vertices.iter() {
                output_vertices[face].push(vertex.instantiate(low_corner.to_vec(), lighting));
            }
        }
    }
}

/// RGBA color data accepted by [`TextureAllocator`].
pub type Texel = (u8, u8, u8, u8);

/// Allocator of 2D textures (or rather, typically regions in a texture atlas) to paint
/// block faces into. Implement this trait using the target graphics API's texture type.
pub trait TextureAllocator {
    /// Tile handles produced by this allocator.
    type Tile: TextureTile;

    /// Edge length of the texture tiles
    fn resolution(&self) -> GridCoordinate;

    /// Allocate a tile, whose texture coordinates will be available as long as the `Tile`
    /// value, and its clones, are not dropped.
    fn allocate(&mut self) -> Self::Tile;
}

/// 2D texture to paint block faces into. It is assumed that when this value is dropped,
/// the texture allocation will be released.
pub trait TextureTile: Clone {
    /// Transform a unit-square texture coordinate for the tile ([0..1] in each
    /// component) into a general texture coordinate.
    fn texcoord(&self, in_tile: Vector2<TextureCoordinate>) -> Vector3<TextureCoordinate>;

    /// Write texture data as RGBA color.
    ///
    /// `data` must be of length `allocator.resolution().pow(2)`.
    fn write(&mut self, data: &[Texel]);
}

/// [`TextureAllocator`] which discards all input except for counting calls; for testing.
///
/// This type is public so that it may be used in benchmarks and such.
#[derive(Debug, Eq, PartialEq)]
pub struct TestTextureAllocator {
    resolution: GridCoordinate,
    count_allocated: usize,
}

impl TestTextureAllocator {
    pub fn new(resolution: Resolution) -> Self {
        Self {
            resolution: resolution.into(),
            count_allocated: 0,
        }
    }

    /// Number of tiles allocated. Does not decrement for deallocations.
    pub fn count_allocated(&self) -> usize {
        self.count_allocated
    }
}

impl TextureAllocator for TestTextureAllocator {
    type Tile = TestTextureTile;

    fn resolution(&self) -> GridCoordinate {
        self.resolution
    }

    fn allocate(&mut self) -> Self::Tile {
        self.count_allocated += 1;
        TestTextureTile {
            data_length: usize::try_from(self.resolution()).unwrap().pow(2),
        }
    }
}

/// Tile type for [`TestTextureAllocator`].
///
/// This type is public so that it may be used in benchmarks and such.
#[derive(Clone, Debug)]
pub struct TestTextureTile {
    data_length: usize,
}

impl TextureTile for TestTextureTile {
    fn texcoord(&self, in_tile: Vector2<TextureCoordinate>) -> Vector3<TextureCoordinate> {
        in_tile.extend(0.0)
    }

    fn write(&mut self, data: &[(u8, u8, u8, u8)]) {
        // Validate data size.
        assert_eq!(
            data.len(),
            self.data_length,
            "tile data did not match resolution"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{Block, BlockAttributes};
    use crate::blockgen::make_some_blocks;
    use crate::math::GridPoint;
    use crate::universe::Universe;
    use cgmath::MetricSpace as _;

    #[test]
    fn excludes_interior_faces() {
        let block = make_some_blocks(1).swap_remove(0);
        let mut space = Space::empty_positive(2, 2, 2);
        space.fill(space.grid(), |_| Some(&block)).unwrap();

        let mut rendering = new_space_buffer();
        triangulate_space::<BlockVertex, BlockVertex, TestTextureTile>(
            &space,
            space.grid(),
            &triangulate_blocks(&space, &mut TestTextureAllocator::new(43)),
            &mut rendering,
        );
        let rendering_flattened: Vec<BlockVertex> = rendering
            .values()
            .iter()
            .flat_map(|r| (*r).clone())
            .collect();
        assert_eq!(
            Vec::<&BlockVertex>::new(),
            rendering_flattened
                .iter()
                .filter(|vertex| vertex.position.distance2(Point3::new(1.0, 1.0, 1.0)) < 0.99)
                .collect::<Vec<&BlockVertex>>(),
            "found an interior point"
        );
        assert_eq!(
            rendering_flattened.len(),
            6 /* vertices per face */
            * 4 /* block faces per exterior side of space */
            * 6, /* sides of space */
            "wrong number of faces"
        );
    }

    #[test]
    fn no_panic_on_missing_blocks() {
        let block = make_some_blocks(1).swap_remove(0);
        let mut space = Space::empty_positive(2, 1, 1);
        let block_triangulations: BlockTriangulations<BlockVertex, _> =
            triangulate_blocks(&space, &mut TestTextureAllocator::new(43));
        assert_eq!(block_triangulations.len(), 1); // check our assumption

        // This should not panic; visual glitches are preferable to failure.
        space.set((0, 0, 0), &block).unwrap(); // render data does not know about this
        triangulate_space(
            &space,
            space.grid(),
            &block_triangulations,
            &mut new_space_buffer(),
        );
    }

    /// Construct a 1x1 recursive block and test that this is equivalent in geometry
    /// to an atom block.
    #[test]
    fn trivial_subcube_rendering() {
        let mut u = Universe::new();
        let mut inner_block_space = Space::empty_positive(1, 1, 1);
        inner_block_space
            .set((0, 0, 0), &make_some_blocks(1)[0])
            .unwrap();
        let inner_block = Block::Recur {
            attributes: BlockAttributes::default(),
            offset: GridPoint::origin(),
            resolution: 1,
            space: u.insert_anonymous(inner_block_space),
        };
        let mut outer_space = Space::empty_positive(1, 1, 1);
        outer_space.set((0, 0, 0), &inner_block).unwrap();

        let mut tex = TestTextureAllocator::new(1);
        let block_triangulations: BlockTriangulations<BlockVertex, _> =
            triangulate_blocks(&outer_space, &mut tex);
        let block_render_data: BlockTriangulation<_, _> = block_triangulations[0].clone();

        eprintln!("{:#?}", block_triangulations);
        let mut space_rendered = new_space_buffer();
        triangulate_space(
            &outer_space,
            outer_space.grid(),
            &block_triangulations,
            &mut space_rendered,
        );
        eprintln!("{:#?}", space_rendered);

        assert_eq!(
            space_rendered,
            block_render_data.faces.map(|_, frd| frd.vertices.to_vec())
        );
        assert_eq!(
            tex.count_allocated(),
            6,
            "Should be only 6 cube face textures"
        );
    }

    /// Check for hidden surfaces being given textures.
    #[test]
    fn no_extraneous_layers() {
        let resolution = 8;
        let mut u = Universe::new();
        let mut inner_block_space = Space::empty(Grid::for_block(resolution));
        let filler_block = make_some_blocks(1).swap_remove(0);
        inner_block_space
            .fill(Grid::new((2, 2, 2), (4, 4, 4)), |_| Some(&filler_block))
            .unwrap();
        let inner_block = Block::Recur {
            attributes: BlockAttributes::default(),
            offset: GridPoint::origin(),
            resolution: 16,
            space: u.insert_anonymous(inner_block_space),
        };
        let mut outer_space = Space::empty_positive(1, 1, 1);
        outer_space.set((0, 0, 0), &inner_block).unwrap();

        let mut tex = TestTextureAllocator::new(resolution);
        let block_triangulations: BlockTriangulations<BlockVertex, _> =
            triangulate_blocks(&outer_space, &mut tex);

        eprintln!("{:#?}", block_triangulations);
        let mut space_rendered = new_space_buffer();
        triangulate_space(
            &outer_space,
            outer_space.grid(),
            &block_triangulations,
            &mut space_rendered,
        );
        eprintln!("{:#?}", space_rendered);

        assert_eq!(
            tex.count_allocated(),
            6,
            "Should be only 6 cube face textures"
        );
    }

    #[test]
    fn test_texture_allocator() {
        let mut allocator = TestTextureAllocator::new(123);
        assert_eq!(allocator.resolution(), 123);
        assert_eq!(allocator.count_allocated(), 0);
        let _ = allocator.allocate();
        let _ = allocator.allocate();
        assert_eq!(allocator.count_allocated(), 2);
    }

    // TODO: more tests
}
