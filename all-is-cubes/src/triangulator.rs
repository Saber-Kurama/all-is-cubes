// Copyright 2020-2021 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

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

use cgmath::{
    ElementWise as _, EuclideanSpace as _, Point2, Point3, Transform as _, Vector2, Vector3,
};
use std::convert::TryFrom;

use crate::block::{evaluated_block_resolution, EvaluatedBlock, Evoxel, Resolution};
use crate::content::palette;
use crate::math::{Face, FaceMap, FreeCoordinate, GridCoordinate, Rgba};
use crate::space::{BlockIndex, Grid, PackedLight, Space};
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
    /// Vertex normal, always axis-aligned.
    pub face: Face,
    /// Surface color or texture coordinate.
    pub coloring: Coloring,
}
/// Describes the two ways a [`BlockVertex`] may be colored; by a solid color or by a texture.
#[derive(Clone, Copy, PartialEq)]
pub enum Coloring {
    /// Solid color.
    Solid(Rgba),
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
            self.face.normal_vector::<i8>().as_concise_debug(), // no decimals!
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

const QUAD_VERTICES: &[Vector2<FreeCoordinate>; 6] = &[
    // Two-triangle quad.
    // Note that looked at from a X-right Y-up view, these triangles are
    // clockwise, but they're properly counterclockwise from the perspective
    // that we're drawing the face _facing towards negative Z_ (into the screen),
    // which is how cube faces as implicitly defined by Face::matrix work.
    Vector2::new(0.0, 0.0),
    Vector2::new(0.0, 1.0),
    Vector2::new(1.0, 0.0),
    Vector2::new(1.0, 0.0),
    Vector2::new(0.0, 1.0),
    Vector2::new(1.0, 1.0),
];

#[inline]
fn push_quad<V: From<BlockVertex>>(
    vertices: &mut Vec<V>,
    face: Face,
    depth: FreeCoordinate,
    low_corner: Point2<FreeCoordinate>,
    high_corner: Point2<FreeCoordinate>,
    coloring: QuadColoring<impl TextureTile>,
) {
    let transform = face.matrix(1).to_free();
    for &p in QUAD_VERTICES {
        // Apply bounding rectangle
        let p = low_corner.to_vec() + p.mul_element_wise(high_corner - low_corner);
        // Apply depth
        let p = Point3::from_vec(p.extend(depth));

        vertices.push(V::from(BlockVertex {
            position: transform.transform_point(p),
            face,
            coloring: match coloring {
                // Note: if we're ever looking for microöptimizations, we could try
                // converting this to a trait for static dispatch.
                QuadColoring::Solid(color) => Coloring::Solid(color),
                QuadColoring::Texture(tile, scale) => {
                    Coloring::Texture(tile.texcoord(Vector2::new(
                        p.x as TextureCoordinate * scale,
                        p.y as TextureCoordinate * scale,
                    )))
                }
            },
        }));
    }
}

/// Helper for [`push_quad`] which offers the alternatives of solid color or texturing.
/// Compared to [`Coloring`], it describes texturing for an entire quad rather than a vertex.
#[derive(Copy, Clone, Debug)]
enum QuadColoring<'a, T> {
    Solid(Rgba),
    Texture(&'a T, TextureCoordinate),
}

/// Generate [`BlockTriangulation`] for a block.
pub fn triangulate_block<V: From<BlockVertex>, A: TextureAllocator>(
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

                FaceTriangulation {
                    fully_opaque: block.color.fully_opaque(),
                    vertices: if !block.color.fully_transparent() {
                        let mut face_vertices: Vec<V> = Vec::with_capacity(6);
                        push_quad(
                            &mut face_vertices,
                            face,
                            /* depth= */ 0.,
                            Point2 { x: 0., y: 0. },
                            Point2 { x: 1., y: 1. },
                            QuadColoring::<A::Tile>::Solid(block.color),
                        );
                        face_vertices
                    } else {
                        Vec::new()
                    },
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
            let tile_resolution: GridCoordinate = texture_allocator.resolution();
            let mut block_resolution = match evaluated_block_resolution(voxels.grid()) {
                Some(r) => GridCoordinate::from(r),
                // TODO: return an invalid block marker.
                None => return BlockTriangulation::default(),
            };

            // TODO: Temporarily implementing only the lower-resolution case
            // To implement higher resolution, refactor so that we first generate an arbitrary size
            // texture, then slice or pad it as necessary.
            if block_resolution > tile_resolution {
                block_resolution = tile_resolution;
            }

            for &face in Face::ALL_SIX {
                let transform = face.matrix(block_resolution - 1);

                // Layer 0 is the outside surface of the cube and successive layers are
                // deeper inside.
                for layer in 0..block_resolution {
                    // TODO: JS version would detect fully-opaque blocks (a derived property of Block)
                    // and only scan the first and last faces
                    let mut tile_texels: Vec<Texel> =
                        Vec::with_capacity((tile_resolution as usize).pow(2));
                    let mut layer_is_visible_somewhere = false;

                    // Track the bounding box of the layer that's actually occupied.
                    // Uses inclusive-exclusive coordinates.
                    // Invariant: If layer_is_visible_somewhere, then visible_low_corner < visible_high_corner.
                    let mut visible_low_corner = Point2::new(block_resolution, block_resolution);
                    let mut visible_high_corner = Point2::new(0, 0);

                    for t in 0..block_resolution {
                        for s in 0..block_resolution {
                            let texel_coord = Point2::new(s, t);
                            // TODO: Matrix4 isn't allowed to be integer. Make Face provide a better strategy.
                            // While we're at it, also implement the optimization that positive and negative
                            // faces can share a texture sometimes (which requires dropping the property
                            // Face::matrix provides where all transforms contain no mirroring).
                            let cube: Point3<GridCoordinate> =
                                transform.transform_point(Point3::new(s, t, layer));

                            // Diagnose out-of-space accesses. TODO: Tidy this up and document it, or remove it:
                            // it will happen whenever the space is the wrong size for the textures.
                            let color = voxels
                                .get(cube)
                                .unwrap_or(&Evoxel::new(palette::MISSING_VOXEL_FALLBACK))
                                .color;

                            if !color.fully_transparent() && {
                                // Compute whether this voxel is not hidden behind another
                                let obscuring_cube = cube + face.normal_vector();
                                !voxels
                                    .get(obscuring_cube)
                                    .map(|ev| ev.color.fully_opaque())
                                    .unwrap_or(false)
                            } {
                                layer_is_visible_somewhere = true;
                                for axis in 0..2 {
                                    visible_low_corner[axis] =
                                        visible_low_corner[axis].min(texel_coord[axis]);
                                    visible_high_corner[axis] =
                                        visible_high_corner[axis].max(texel_coord[axis] + 1);
                                }
                            }

                            if layer == 0 && !color.fully_opaque() {
                                // If the first layer is transparent somewhere...
                                output_by_face[face].fully_opaque = false;
                            }

                            tile_texels.push(color.to_linear_32bit());
                        }
                        if block_resolution < tile_resolution {
                            // Pad texture out
                            let last_color = *tile_texels.last().unwrap();
                            tile_texels
                                .extend((block_resolution..tile_resolution).map(|_| last_color));
                        }
                    }
                    if block_resolution < tile_resolution {
                        // Pad texture out
                        let last_row = Vec::from(
                            &tile_texels[(tile_texels.len() - tile_resolution as usize)..],
                        );
                        for _ in block_resolution..tile_resolution {
                            tile_texels.extend(&last_row);
                        }
                    }
                    debug_assert_eq!(
                        tile_texels.len(),
                        (tile_resolution * tile_resolution) as usize
                    );

                    // TODO: To reduce artifacts, process tile_texels to add an anti-bleed border
                    // outside of the visible rectangle. (See if we can reuse the texture allocator
                    // blitting code.)

                    if layer_is_visible_somewhere {
                        // Actually store and use the texels we just computed.
                        // Only the surface faces go anywhere but WITHIN.
                        let face_vertices = &mut output_by_face
                            [if layer == 0 { face } else { Face::WITHIN }]
                        .vertices;
                        let depth =
                            FreeCoordinate::from(layer) / FreeCoordinate::from(block_resolution);

                        let mut maybe_texture_tile = None;
                        let coloring = if let Some(uniform_color) = rectangle_is_uniform_color(
                            &tile_texels,
                            tile_resolution,
                            visible_low_corner,
                            visible_high_corner,
                        ) {
                            // The quad we're going to draw has identical texels, so we might as
                            // well use a solid color and skip allocating a texture tile.
                            QuadColoring::<A::Tile>::Solid(Rgba::from_linear_32bit(uniform_color))
                        } else {
                            maybe_texture_tile = texture_allocator.allocate();
                            if let Some(ref mut texture_tile) = maybe_texture_tile {
                                texture_tile.write(tile_texels.as_ref());
                                QuadColoring::Texture(
                                    texture_tile,
                                    block_resolution as TextureCoordinate
                                        / tile_resolution as TextureCoordinate,
                                )
                            } else {
                                // Texture allocation failure.
                                // TODO: Mark this triangulation as defective in the return value, so
                                // that when more space is available, it can be retried, rather than
                                // having lingering failures.
                                // TODO: Add other fallback strategies such as using vertices instead
                                // of textures.
                                QuadColoring::Solid(palette::MISSING_TEXTURE_FALLBACK)
                            }
                        };

                        push_quad(
                            face_vertices,
                            face,
                            depth,
                            visible_low_corner.map(|c| {
                                FreeCoordinate::from(c) / FreeCoordinate::from(block_resolution)
                            }),
                            visible_high_corner.map(|c| {
                                FreeCoordinate::from(c) / FreeCoordinate::from(block_resolution)
                            }),
                            coloring,
                        );
                        textures_used.extend(maybe_texture_tile);
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

fn rectangle_is_uniform_color(
    texels: &[Texel],
    resolution: i32,
    low_corner: Point2<i32>,
    high_corner: Point2<i32>,
) -> Option<Texel> {
    let mut first = None;
    for y in low_corner.y..high_corner.y {
        // a.k.a. texture coordinate s
        let row: usize = y as usize * resolution as usize;
        for x in low_corner.x..high_corner.x {
            // a.k.a. texture coordinate t
            let index: usize = row + x as usize;
            let texel = texels[index];
            if texel != *first.get_or_insert(texel) {
                return None;
            }
        }
    }
    first
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

/// Container for a triangle-based representation of a [`Space`] (or part of it) which may
/// then be rasterized.
///
/// A `SpaceTriangulation` may be used multiple times as a [`Space`] is modified.
/// Currently, the only benefit of this is avoiding reallocating memory.
///
/// Type parameter `GV` is the type of triangle vertices.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceTriangulation<GV> {
    // TODO: This struct is going to expand by having indices and by splitting indices into groups
    // for opacity and depth sorting.
    vertices: Vec<GV>,
}

impl<GV> SpaceTriangulation<GV> {
    /// Construct an empty `SpaceTriangulation` which draws nothing.
    pub const fn new() -> Self {
        Self {
            vertices: Vec::new(),
        }
    }

    /// Shorthand for <code>[Self::new()].[compute](Self::compute)(...)</code>.
    pub fn triangulate<'p, BV, T, P>(space: &Space, bounds: Grid, block_triangulations: P) -> Self
    where
        BV: ToGfxVertex<GV> + 'p,
        P: BlockTriangulationProvider<'p, BV, T>,
        T: 'p,
    {
        let mut this = Self::new();
        this.compute(space, bounds, block_triangulations);
        this
    }

    /// Computes triangles for the contents of `space` within `bounds` and stores them
    /// in `self`.
    ///
    /// `block_triangulations` should be the result of [`triangulate_blocks`] or equivalent,
    /// and must be up-to-date with the [`Space`]'s blocks or the result will be inaccurate
    /// and may contain severe lighting errors.
    ///
    /// Note about edge case behavior: This algorithm does not use the [`Space`]'s block data
    /// at all. Thus, it always has a consistent interpretation based on
    /// `block_triangulations` (as opposed to, for example, using face opacity data not the
    /// same as the meshes and thus producing a rendering with gaps in it).
    pub fn compute<'p, BV, T, P>(
        &mut self,
        space: &Space,
        bounds: Grid,
        mut block_triangulations: P,
    ) where
        BV: ToGfxVertex<GV> + 'p,
        P: BlockTriangulationProvider<'p, BV, T>,
        T: 'p,
    {
        // TODO: On out-of-range, draw an obviously invalid block instead of an invisible one?
        // If we do this, we'd make it the provider's responsibility
        let empty_render = BlockTriangulation::<BV, T>::default();

        // use the buffer but not the existing data
        self.vertices.clear();

        for cube in bounds.interior_iter() {
            let precomputed = space
                .get_block_index(cube)
                .and_then(|index| block_triangulations.get(index))
                .unwrap_or(&empty_render);
            let low_corner = cube.cast::<BV::Coordinate>().unwrap();
            for &face in Face::ALL_SEVEN {
                let adjacent_cube = cube + face.normal_vector();
                if space
                    .get_block_index(adjacent_cube)
                    .and_then(|index| block_triangulations.get(index))
                    .map(|bt| bt.faces[face.opposite()].fully_opaque)
                    .unwrap_or(false)
                {
                    // Don't draw obscured faces
                    continue;
                }

                let lighting = space.get_lighting(adjacent_cube);

                // Copy vertices, offset to the block position and with lighting
                for vertex in precomputed.faces[face].vertices.iter() {
                    self.vertices
                        .push(vertex.instantiate(low_corner.to_vec(), lighting));
                }
            }
        }
    }

    pub fn vertices(&self) -> &[GV] {
        &self.vertices
    }
}

impl<GV> Default for SpaceTriangulation<GV> {
    fn default() -> Self {
        Self::new()
    }
}

/// Source of [`BlockTriangulation`] values for [`SpaceTriangulation::compute`].
///
/// This trait allows the caller of [`SpaceTriangulation::compute`] to provide an
/// implementation which records which blocks were actually used, for precise
/// invalidation.
pub trait BlockTriangulationProvider<'a, V, T> {
    fn get(&mut self, index: BlockIndex) -> Option<&'a BlockTriangulation<V, T>>;
}
impl<'a, V, T> BlockTriangulationProvider<'a, V, T> for &'a [BlockTriangulation<V, T>] {
    fn get(&mut self, index: BlockIndex) -> Option<&'a BlockTriangulation<V, T>> {
        <[_]>::get(self, usize::from(index))
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
    ///
    /// Returns `None` if no space is available for another tile.
    fn allocate(&mut self) -> Option<Self::Tile>;
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
    capacity: usize,
    count_allocated: usize,
}

impl TestTextureAllocator {
    pub fn new(resolution: Resolution) -> Self {
        Self {
            resolution: resolution.into(),
            capacity: usize::MAX,
            count_allocated: 0,
        }
    }

    /// Fail after allocating this many tiles. (Currently does not track deallocations.)
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
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

    fn allocate(&mut self) -> Option<Self::Tile> {
        if self.count_allocated == self.capacity {
            None
        } else {
            self.count_allocated += 1;
            Some(TestTextureTile {
                data_length: usize::try_from(self.resolution()).unwrap().pow(2),
            })
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
    use crate::block::{Block, BlockAttributes, AIR};
    use crate::content::make_some_blocks;
    use crate::math::{Face::*, GridPoint};
    use crate::universe::Universe;
    use cgmath::MetricSpace as _;

    /// Shorthand for writing out an entire [`BlockVertex`] with solid color.
    fn v_c(position: [FreeCoordinate; 3], face: Face, color: [f32; 4]) -> BlockVertex {
        BlockVertex {
            position: position.into(),
            face,
            coloring: Coloring::Solid(Rgba::new(color[0], color[1], color[2], color[3])),
        }
    }

    /// Shorthand for writing out an entire [`BlockVertex`] with texturing.
    fn v_t(
        position: [FreeCoordinate; 3],
        face: Face,
        texture: [TextureCoordinate; 3],
    ) -> BlockVertex {
        BlockVertex {
            position: position.into(),
            face,
            coloring: Coloring::Texture(texture.into()),
        }
    }

    /// Test helper to call `triangulate_block` alone without a `Space`.
    fn test_triangulate_block(block: Block) -> BlockTriangulation<BlockVertex, TestTextureTile> {
        let triangulation = triangulate_block(
            &block.evaluate().unwrap(),
            &mut TestTextureAllocator::new(16),
        );
        triangulation
    }

    /// Test helper to call `triangulate_blocks` followed directly by `triangulate_space`.
    fn triangulate_blocks_and_space(
        space: &Space,
        texture_resolution: Resolution,
    ) -> (
        TestTextureAllocator,
        BlockTriangulations<BlockVertex, TestTextureTile>,
        SpaceTriangulation<BlockVertex>,
    ) {
        let mut tex = TestTextureAllocator::new(texture_resolution);
        let block_triangulations = triangulate_blocks(space, &mut tex);
        let space_triangulation = SpaceTriangulation::<BlockVertex>::triangulate::<
            BlockVertex,
            TestTextureTile,
            _,
        >(space, space.grid(), &*block_triangulations);
        (tex, block_triangulations, space_triangulation)
    }

    fn non_uniform_fill(cube: GridPoint) -> &'static Block {
        const BLOCKS: &[Block] = &[
            Block::Atom(BlockAttributes::default(), rgba_const!(1., 1., 1., 1.)),
            Block::Atom(BlockAttributes::default(), rgba_const!(0., 0., 0., 1.)),
        ];
        &BLOCKS[(cube.x + cube.y + cube.z).rem_euclid(2) as usize]
    }

    #[test]
    fn excludes_hidden_faces_of_blocks() {
        let mut space = Space::empty_positive(2, 2, 2);
        space
            .fill(space.grid(), |p| Some(non_uniform_fill(p)))
            .unwrap();
        let (_, _, space_tri) = triangulate_blocks_and_space(&space, 7);

        // The space rendering should be a 2×2×2 cube of tiles, without any hidden interior faces.
        assert_eq!(
            Vec::<&BlockVertex>::new(),
            space_tri
                .vertices()
                .iter()
                .filter(|vertex| vertex.position.distance2(Point3::new(1.0, 1.0, 1.0)) < 0.99)
                .collect::<Vec<&BlockVertex>>(),
            "found an interior point"
        );
        assert_eq!(
            space_tri.vertices().len(),
            6 /* vertices per face */
            * 4 /* block faces per exterior side of space */
            * 6, /* sides of space */
            "wrong number of faces"
        );
    }

    /// Run [`triangulate_space`] with stale block data and confirm it does not panic.
    #[test]
    fn no_panic_on_missing_blocks() {
        let block = make_some_blocks(1).swap_remove(0);
        let mut space = Space::empty_positive(2, 1, 1);
        let block_triangulations: BlockTriangulations<BlockVertex, _> =
            triangulate_blocks(&space, &mut TestTextureAllocator::new(43));
        assert_eq!(block_triangulations.len(), 1); // check our assumption

        // This should not panic; visual glitches are preferable to failure.
        space.set((0, 0, 0), &block).unwrap(); // render data does not know about this
        SpaceTriangulation::triangulate(&space, space.grid(), &*block_triangulations);
    }

    /// Construct a 1x1 recursive block and test that this is equivalent in geometry
    /// to an atom block.
    #[test]
    fn trivial_voxels_equals_atom() {
        // Construct recursive block.
        let mut u = Universe::new();
        let atom_block = Block::from(Rgba::new(0.0, 1.0, 0.0, 1.0));
        let trivial_recursive_block = Block::builder()
            .voxels_fn(&mut u, 1, |_| &atom_block)
            .unwrap()
            .build();

        let (_, _, space_rendered_a) = triangulate_blocks_and_space(
            &{
                let mut space = Space::empty_positive(1, 1, 1);
                space.set((0, 0, 0), &atom_block).unwrap();
                space
            },
            1,
        );
        let (tex, _, space_rendered_r) = triangulate_blocks_and_space(
            &{
                let mut space = Space::empty_positive(1, 1, 1);
                space.set((0, 0, 0), &trivial_recursive_block).unwrap();
                space
            },
            1,
        );

        assert_eq!(space_rendered_a, space_rendered_r);
        assert_eq!(tex.count_allocated(), 0);
    }

    /// [`triangulate_space`] of a 1×1×1 space has the same geometry as the contents.
    #[test]
    fn space_tri_equals_block_tri() {
        // Construct recursive block.
        let mut u = Universe::new();
        let mut blocks = make_some_blocks(2);
        blocks.push(AIR);
        let recursive_block = Block::builder()
            .voxels_fn(&mut u, 4, |p| {
                &blocks[(p.x as usize).rem_euclid(blocks.len())]
            })
            .unwrap()
            .build();
        let mut outer_space = Space::empty_positive(1, 1, 1);
        outer_space.set((0, 0, 0), &recursive_block).unwrap();

        let (tex, block_triangulations, space_rendered) =
            triangulate_blocks_and_space(&outer_space, 1);

        eprintln!("{:#?}", block_triangulations);
        eprintln!("{:#?}", space_rendered);

        assert_eq!(
            space_rendered
                .vertices()
                .into_iter()
                .copied()
                .collect::<Vec<_>>(),
            block_triangulations[0]
                .faces
                .values()
                .iter()
                .flat_map(|face_render| face_render.vertices.clone().into_iter())
                .collect::<Vec<_>>()
        );
        assert_eq!(tex.count_allocated(), 0);
    }

    #[test]
    fn block_resolution_less_than_tile() {
        let block_resolution = 4;
        let tile_resolution = 8;
        let mut u = Universe::new();
        let block = Block::builder()
            .voxels_fn(&mut u, block_resolution, non_uniform_fill)
            .unwrap()
            .build();
        let mut outer_space = Space::empty_positive(1, 1, 1);
        outer_space.set((0, 0, 0), &block).unwrap();

        let (_, _, _) = triangulate_blocks_and_space(&outer_space, tile_resolution);
        // TODO: Figure out how to make a useful assert. At least this is "it doesn't panic".
    }

    #[test]
    fn block_resolution_greater_than_tile() {
        let block_resolution = 8;
        let tile_resolution = 4;
        let mut u = Universe::new();
        let block = Block::builder()
            .voxels_fn(&mut u, block_resolution, non_uniform_fill)
            .unwrap()
            .build();
        let mut outer_space = Space::empty_positive(1, 1, 1);
        outer_space.set((0, 0, 0), &block).unwrap();

        let (_, _, _) = triangulate_blocks_and_space(&outer_space, tile_resolution);
        // TODO: Figure out how to make a useful assert. At least this is "it doesn't panic".
    }

    /// Check for hidden surfaces being given textures.
    /// Exercise the “shrinkwrap” logic that generates geometry no larger than necessary.
    #[test]
    #[rustfmt::skip]
    fn shrunken_box_has_no_extras() {
        // Construct a box whose faces don't touch the outer extent of the volume.
        let resolution = 8;
        let mut u = Universe::new();
        let less_than_full_block = Block::builder()
            .voxels_fn(&mut u, resolution, |cube| {
                if Grid::new((2, 2, 2), (4, 4, 4)).contains_cube(cube) {
                    non_uniform_fill(cube)
                } else {
                    &AIR
                }
            })
            .unwrap()
            .build();
        let mut outer_space = Space::empty_positive(1, 1, 1);
        outer_space.set((0, 0, 0), &less_than_full_block).unwrap();

        let (tex, _, space_rendered) = triangulate_blocks_and_space(&outer_space, resolution);

        assert_eq!(
            tex.count_allocated(),
            6,
            "Should be only 6 cube face textures"
        );
        assert_eq!(
            space_rendered.vertices().into_iter().cloned().collect::<Vec<_>>(),
            vec![
                v_t([0.250, 0.250, 0.250], NX, [0.250, 0.250, 0.000]),
                v_t([0.250, 0.250, 0.750], NX, [0.250, 0.750, 0.000]),
                v_t([0.250, 0.750, 0.250], NX, [0.750, 0.250, 0.000]),
                v_t([0.250, 0.750, 0.250], NX, [0.750, 0.250, 0.000]),
                v_t([0.250, 0.250, 0.750], NX, [0.250, 0.750, 0.000]),
                v_t([0.250, 0.750, 0.750], NX, [0.750, 0.750, 0.000]),
                v_t([0.250, 0.250, 0.250], NY, [0.250, 0.250, 0.000]),
                v_t([0.750, 0.250, 0.250], NY, [0.250, 0.750, 0.000]),
                v_t([0.250, 0.250, 0.750], NY, [0.750, 0.250, 0.000]),
                v_t([0.250, 0.250, 0.750], NY, [0.750, 0.250, 0.000]),
                v_t([0.750, 0.250, 0.250], NY, [0.250, 0.750, 0.000]),
                v_t([0.750, 0.250, 0.750], NY, [0.750, 0.750, 0.000]),
                v_t([0.250, 0.250, 0.250], NZ, [0.250, 0.250, 0.000]),
                v_t([0.250, 0.750, 0.250], NZ, [0.250, 0.750, 0.000]),
                v_t([0.750, 0.250, 0.250], NZ, [0.750, 0.250, 0.000]),
                v_t([0.750, 0.250, 0.250], NZ, [0.750, 0.250, 0.000]),
                v_t([0.250, 0.750, 0.250], NZ, [0.250, 0.750, 0.000]),
                v_t([0.750, 0.750, 0.250], NZ, [0.750, 0.750, 0.000]),
                v_t([0.750, 0.750, 0.250], PX, [0.250, 0.250, 0.000]),
                v_t([0.750, 0.750, 0.750], PX, [0.250, 0.750, 0.000]),
                v_t([0.750, 0.250, 0.250], PX, [0.750, 0.250, 0.000]),
                v_t([0.750, 0.250, 0.250], PX, [0.750, 0.250, 0.000]),
                v_t([0.750, 0.750, 0.750], PX, [0.250, 0.750, 0.000]),
                v_t([0.750, 0.250, 0.750], PX, [0.750, 0.750, 0.000]),
                v_t([0.750, 0.750, 0.250], PY, [0.250, 0.250, 0.000]),
                v_t([0.250, 0.750, 0.250], PY, [0.250, 0.750, 0.000]),
                v_t([0.750, 0.750, 0.750], PY, [0.750, 0.250, 0.000]),
                v_t([0.750, 0.750, 0.750], PY, [0.750, 0.250, 0.000]),
                v_t([0.250, 0.750, 0.250], PY, [0.250, 0.750, 0.000]),
                v_t([0.250, 0.750, 0.750], PY, [0.750, 0.750, 0.000]),
                v_t([0.250, 0.750, 0.750], PZ, [0.250, 0.250, 0.000]),
                v_t([0.250, 0.250, 0.750], PZ, [0.250, 0.750, 0.000]),
                v_t([0.750, 0.750, 0.750], PZ, [0.750, 0.250, 0.000]),
                v_t([0.750, 0.750, 0.750], PZ, [0.750, 0.250, 0.000]),
                v_t([0.250, 0.250, 0.750], PZ, [0.250, 0.750, 0.000]),
                v_t([0.750, 0.250, 0.750], PZ, [0.750, 0.750, 0.000]),
            ],
        );
    }

    /// Exercise the case where textures are skipped because the color is uniform.
    /// TODO: There are more subcases such as still using textures for irregular
    /// shapes.
    #[test]
    #[rustfmt::skip]
    fn shrunken_box_uniform_color() {
        // Construct a box whose faces don't touch the outer extent of the volume.
        let resolution = 8;
        let mut u = Universe::new();
        let filler_block = Block::Atom(
            BlockAttributes::default(),
            // Caution: the conversion involves a round trip through 8-bit color.
            // This exercises that rounding consciously but we might decide to
            // get rid of it.
            Rgba::new(0.0, 1.0, 0.5, 1.0),
        );
        let less_than_full_block = Block::builder()
            .voxels_fn(&mut u, resolution, |cube| {
                if Grid::new((2, 2, 2), (4, 4, 4)).contains_cube(cube) {
                    &filler_block
                } else {
                    &AIR
                }
            })
            .unwrap()
            .build();
        let mut outer_space = Space::empty_positive(1, 1, 1);
        outer_space.set((0, 0, 0), &less_than_full_block).unwrap();

        let (tex, _, space_rendered) = triangulate_blocks_and_space(&outer_space, resolution);

        assert_eq!(tex.count_allocated(), 0, "Should be no cube face textures");
        assert_eq!(
            space_rendered.vertices().into_iter().cloned().collect::<Vec<_>>(),
            vec![
                v_c([0.250, 0.250, 0.250], NX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.250, 0.750], NX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.250], NX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.250], NX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.250, 0.750], NX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.750], NX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.250, 0.250], NY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.250], NY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.250, 0.750], NY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.250, 0.750], NY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.250], NY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.750], NY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.250, 0.250], NZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.250], NZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.250], NZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.250], NZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.250], NZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.250], NZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.250], PX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.750], PX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.250], PX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.250], PX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.750], PX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.750], PX, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.250], PY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.250], PY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.750], PY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.750], PY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.250], PY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.750], PY, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.750, 0.750], PZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.250, 0.750], PZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.750], PZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.750, 0.750], PZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.250, 0.250, 0.750], PZ, [0.0, 1.0, 0.49803922, 1.0]),
                v_c([0.750, 0.250, 0.750], PZ, [0.0, 1.0, 0.49803922, 1.0]),
            ],
        );
    }

    /// Make a `FaceMap` with uniform values except for `WITHIN`.
    fn except_within<T: Clone>(without: T, within: T) -> FaceMap<T> {
        FaceMap::generate(|face| {
            if face == Face::WITHIN {
                within.clone()
            } else {
                without.clone()
            }
        })
    }

    #[test]
    fn fully_opaque_atom() {
        assert_eq!(
            test_triangulate_block(Block::from(Rgba::WHITE))
                .faces
                .map(|_, ft| ft.fully_opaque),
            except_within(true, false)
        );
        assert_eq!(
            test_triangulate_block(Block::from(Rgba::TRANSPARENT))
                .faces
                .map(|_, ft| ft.fully_opaque),
            except_within(false, false)
        );
        assert_eq!(
            test_triangulate_block(Block::from(Rgba::new(1.0, 1.0, 1.0, 0.5)))
                .faces
                .map(|_, ft| ft.fully_opaque),
            except_within(false, false)
        );
    }

    #[test]
    fn fully_opaque_voxels() {
        let resolution = 8;
        let mut u = Universe::new();
        let block = Block::builder()
            .voxels_fn(&mut u, resolution, |cube| {
                // Make a cube-corner shape
                // TODO: Also test partial alpha
                if cube.x < 1 || cube.y < 1 || cube.z < 1 {
                    Block::from(Rgba::BLACK)
                } else {
                    AIR
                }
            })
            .unwrap()
            .build();
        assert_eq!(
            test_triangulate_block(block)
                .faces
                .map(|_, ft| ft.fully_opaque),
            FaceMap {
                within: false,
                nx: true,
                ny: true,
                nz: true,
                px: false,
                py: false,
                pz: false,
            }
        );
    }

    #[test]
    fn handling_allocation_failure() {
        let resolution = 8;
        let mut u = Universe::new();
        let complex_block = Block::builder()
            .voxels_fn(&mut u, resolution, |cube| {
                if (cube.x + cube.y + cube.z) % 2 == 0 {
                    Rgba::WHITE.into()
                } else {
                    AIR
                }
            })
            .unwrap()
            .build();

        let mut space = Space::empty_positive(1, 1, 1);
        space.set((0, 0, 0), &complex_block).unwrap();

        let mut tex = TestTextureAllocator::new(resolution);
        // Actual capacity needed is resolution * 6, so this will fail.
        let capacity = resolution as usize * 2;
        tex.set_capacity(capacity);
        let block_triangulations: BlockTriangulations<BlockVertex, _> =
            triangulate_blocks(&space, &mut tex);

        // Check results.
        assert_eq!(tex.count_allocated(), capacity);
        assert_eq!(1, block_triangulations.len());
        // TODO: Check that the triangulation includes the failure marker/fallback color.
        let _complex_block_triangulation = &block_triangulations[0];
    }

    /// Test the [`TestTextureAllocator`].
    #[test]
    fn test_texture_allocator() {
        let mut allocator = TestTextureAllocator::new(123);
        assert_eq!(allocator.resolution(), 123);
        assert_eq!(allocator.count_allocated(), 0);
        assert!(allocator.allocate().is_some());
        assert!(allocator.allocate().is_some());
        assert_eq!(allocator.count_allocated(), 2);
        allocator.set_capacity(3);
        assert!(allocator.allocate().is_some());
        assert!(allocator.allocate().is_none());
    }

    // TODO: more tests
}
