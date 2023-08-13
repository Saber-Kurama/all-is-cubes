//! Export to the glTF 3D file format.
//!
//! To use this, create a [`GltfWriter`].
//!
//! TODO: example code here
//!
//! TODO: This is not a clean, well-abstracted library API yet.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;
use std::{fmt, fs, io};

pub use gltf_json as json;
use gltf_json::validation::Checked::Valid;
use gltf_json::Index;

use all_is_cubes::camera::{Camera, Flaws, GraphicsOptions, ViewTransform};
use all_is_cubes::cgmath::One as _;
use all_is_cubes::universe::PartialUniverse;
use all_is_cubes::util::YieldProgress;
use all_is_cubes_mesh::{BlockMesh, MeshOptions, SpaceMesh};

mod buffer;
use buffer::create_buffer_and_accessor;
pub use buffer::GltfDataDestination;
mod animation;
use animation::FrameState;
mod mesh;
use mesh::Materials;
mod glue;
use glue::{convert_quaternion, empty_node, push_and_return_index};
mod texture;
pub use texture::{GltfTextureAllocator, GltfTile};
mod vertex;
pub use vertex::GltfVertex;

use crate::{ExportError, ExportSet};
#[cfg(test)]
mod tests;

/// "This mesh with this translation." A value type that specifies that, in some frame
/// of the output, the particular mesh should be visible at a particular location.
///
/// These are then converted into [`gltf_json::Node`]s with animations controlling when they
/// are visible.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[allow(clippy::exhaustive_structs)]
pub struct MeshInstance {
    /// The mesh to display.
    pub mesh: Index<gltf_json::Mesh>,
    /// Translation applied to this instance of the mesh, in integer (whole block) amounts only.
    pub translation: [i32; 3],
}

/// Handles the construction of [`gltf_json::Root`] and the writing of supporting files
/// for a single glTF asset.
///
/// Life cycle:
/// 1. Create this (providing a [`GltfDataDestination`] for buffers and textures).
/// 2. Call methods to add entities that will be exported.
/// 3. Call [`GltfWriter::into_root()`] to obtain the main
/// [`gltf_json::Root`] value which should be written to the `.gltf` file.
///
/// TODO: Split this struct into "root and buffers" (knows glTF generically) and
/// "scene and animation" (knows how we intend to use it). This will simplify some borrows.
#[derive(Debug)]
pub struct GltfWriter {
    /// Contains all the glTF entities written so far.
    /// Each operation that adds data appends to the vectors of entities inside this.
    /// Entities must not be deleted or reordered, to ensure [`Index`]es stay valid.
    root: gltf_json::Root,

    /// Where to write the buffers and textures.
    buffer_dest: GltfDataDestination,

    /// Testure allocator configured to write to this destination.
    texture_allocator: GltfTextureAllocator,

    /// Materials the meshes need.
    materials: Materials,

    /// glTF camera entity, if created yet.
    /// Its settings are taken from the first [`Camera`] encountered.
    camera: Option<Index<gltf_json::Camera>>,

    /// The state of the world in each frame of the animation.
    frame_states: Vec<FrameState>,

    /// Every mesh appearing anywhere in `frame_states`.
    /// Using BTreeSet for stable ordering.
    any_time_visible_mesh_instances: BTreeSet<MeshInstance>,

    /// All flaws encountered so far.
    flaws: Flaws,
}

impl GltfWriter {
    /// `buffer_dest`: Where to write auxiliary data (vertex buffers, textures).
    pub fn new(buffer_dest: GltfDataDestination) -> Self {
        let mut root = gltf_json::Root {
            asset: gltf_json::Asset {
                generator: Some(String::from("all-is-cubes")),
                ..gltf_json::Asset::default()
            },
            extensions_used: ["KHR_materials_transmission", "KHR_materials_volume"]
                .map(String::from)
                .to_vec(),
            ..gltf_json::Root::default()
        };

        Self {
            materials: Materials::new(&mut root.materials),

            // TODO: Once texturing actually works, enable allocation here.
            texture_allocator: GltfTextureAllocator::new(buffer_dest.clone(), false),

            root,
            buffer_dest,
            camera: None,
            frame_states: Vec::new(),
            any_time_visible_mesh_instances: BTreeSet::new(),
            flaws: Flaws::empty(),
        }
    }

    /// Returns a texture [`Allocator`](all_is_cubes_mesh::texture::Allocator) that writes
    /// textures into this glTF asset
    pub fn texture_allocator(&self) -> GltfTextureAllocator {
        self.texture_allocator.clone()
    }

    /// Add one frame of an animated scene.
    ///
    /// `our_camera` should be the current camera state (its `view_transform`s in
    /// successive frames will be converted into an animation).
    ///
    /// `visible_meshes` is a list of [`MeshInstance`]s that should be visible in the
    /// current frame; the meshes should have been produced by previous calls to
    /// [`GltfWriter::add_mesh()`].
    ///
    /// Returns flaws which come from \[TODO: explain\].
    ///
    /// TODO: This is not a clean API yet; it was designed around the needs of
    /// `all-is-cubes-desktop`'s recording mode.
    pub fn add_frame(
        &mut self,
        our_camera: Option<&Camera>,
        visible_meshes: &[MeshInstance],
    ) -> Flaws {
        // Create camera if and only if one was given and we didn't have one.
        if self.camera.is_none() {
            if let Some(our_camera) = our_camera.as_ref() {
                self.camera = Some(push_and_return_index(
                    &mut self.root.cameras,
                    convert_camera(None, our_camera),
                ));
            }
        }

        self.frame_states.push(FrameState {
            visible_mesh_instances: visible_meshes.to_vec(),
            camera_transform: our_camera
                .map_or_else(ViewTransform::one, |camera| camera.get_view_transform()),
        });
        self.any_time_visible_mesh_instances
            .extend(visible_meshes.iter());

        // TODO: report only flaws from this frame
        self.flaws
    }

    /// Add one [`SpaceMesh`] to the output.
    ///
    /// The mesh's texture allocator must be [`self.texture_allocator()`].
    pub fn add_mesh(
        &mut self,
        name: &dyn fmt::Display,
        mesh: &SpaceMesh<GltfVertex, GltfTile>,
    ) -> Option<Index<gltf_json::Mesh>> {
        // TODO: Deduplicate meshes so that we don't have to store the same data twice if
        // a world change is undone, or in a cyclic animation (or if two chunks have the
        // same contents — once we make chunks in relative coordinates).
        mesh::add_mesh(self, name, mesh)
    }

    /// Finish all scene preparation and return the [`gltf_json::Root`] which is to be
    /// written to a JSON file.
    pub fn into_root(mut self, frame_pace: Duration) -> io::Result<gltf_json::Root> {
        if !self.texture_allocator.is_empty() {
            let _block_texture_index =
                texture::insert_block_texture_atlas(&mut self.root, &self.texture_allocator)?;

            // TODO: Rewrite meshes to have texture coordinates and materials to designate
            // the texture. Otherwise it's useless.
        }

        let mut scene_nodes: Vec<Index<gltf_json::Node>> = Vec::new();

        // If we have a camera entity, create a node for it.
        if let Some(camera_index) = self.camera {
            let mut camera_node = gltf_json::Node {
                camera: Some(camera_index),
                ..empty_node(None)
            };
            if let Some(initial_state) = self.frame_states.get(0) {
                let t = initial_state.camera_transform;
                camera_node.translation = Some(t.disp.map(|c| c as f32).into());
                camera_node.rotation = Some(convert_quaternion(t.rot));
                camera_node.scale = Some([t.scale as f32; 3]);
            }
            let camera_node_index = push_and_return_index(&mut self.root.nodes, camera_node);
            scene_nodes.push(camera_node_index);

            // Generate camera animation
            if self.frame_states.len() > 1 {
                animation::add_camera_animation(&mut self, camera_node_index, frame_pace)?;
            }
        }

        // For each needed mesh instance, create a node with that translation and that mesh.
        let mut instance_nodes: BTreeMap<MeshInstance, Index<gltf_json::Node>> = BTreeMap::new();
        for &instance in self.any_time_visible_mesh_instances.iter() {
            let MeshInstance { mesh, translation } = instance;
            let node_index = push_and_return_index(
                &mut self.root.nodes,
                gltf_json::Node {
                    mesh: Some(mesh),
                    translation: Some(translation.map(|c| c as f32)),
                    // TODO: give this node a name if we can figure out what a good, cheap one is
                    ..empty_node(None)
                },
            );
            instance_nodes.insert(instance, node_index);
            scene_nodes.push(node_index);
        }

        // Add world mesh animations.
        if self.frame_states.len() > 1 {
            // Timeline represented as BTreeMap<node, Vec<(frame number, visibility)>>.
            // The initial state is "visible", so any nonanimated mesh needs no entry.
            let mut timelines: BTreeMap<MeshInstance, Vec<(usize, bool)>> = BTreeMap::new();
            for (frame_number, state) in self.frame_states.iter().enumerate() {
                for &instance in &state.visible_mesh_instances {
                    let timeline = timelines.entry(instance).or_default();
                    if !timeline.last().map_or(true, |&(_, vis)| vis) {
                        // Node needs to be made visible.
                        timeline.push((frame_number, true));
                    }
                }
                // Remove invisible instances (including ones we haven't seen at all yet)
                for &instance in self.any_time_visible_mesh_instances.iter() {
                    if state.visible_mesh_instances.contains(&instance) {
                        // TODO: do a map lookup instead of linear scan?
                        continue;
                    }

                    use std::collections::btree_map::Entry;
                    match timelines.entry(instance) {
                        Entry::Occupied(mut e) => {
                            let timeline = e.get_mut();
                            if timeline.last().map_or(true, |&(_, vis)| vis) {
                                // Node needs to be made invisible.
                                timeline.push((frame_number, false));
                            }
                        }
                        Entry::Vacant(e) => {
                            // Node needs to be made invisible *from the start*.
                            // This should always happen on the first frame
                            assert_eq!(frame_number, 0, "Neglected initial invisibility");
                            e.insert(vec![(frame_number, false)]);
                        }
                    }
                }
            }

            let mut animation_channels = Vec::new();
            let mut animation_samplers = Vec::new();

            for (instance, timeline) in timelines {
                if timeline.is_empty() {
                    continue;
                }
                let node_index = instance_nodes[&instance];
                let time_accessor = create_buffer_and_accessor(
                    &mut self.root,
                    &self.buffer_dest,
                    format!("node {node_index} animation time"),
                    &format!("node-{node_index}-time"),
                    timeline
                        .iter()
                        .map(|&(t, _vis)| [frame_pace.as_secs_f32() * t as f32]),
                )?;
                let scale_accessor = create_buffer_and_accessor(
                    &mut self.root,
                    &self.buffer_dest,
                    format!("node {node_index} visibility"),
                    &format!("node-{node_index}-vis"),
                    timeline
                        .iter()
                        .map(|&(_t, vis)| [f32::from(u8::from(vis)); 3]),
                )?;
                animation_channels.push(gltf_json::animation::Channel {
                    sampler: push_and_return_index(
                        &mut animation_samplers,
                        gltf_json::animation::Sampler {
                            input: time_accessor,
                            interpolation: Valid(gltf_json::animation::Interpolation::Step),
                            output: scale_accessor,
                            extensions: Default::default(),
                            extras: Default::default(),
                        },
                    ),
                    target: gltf_json::animation::Target {
                        node: node_index,
                        path: Valid(gltf_json::animation::Property::Scale),
                        extensions: Default::default(),
                        extras: Default::default(),
                    },
                    extensions: Default::default(),
                    extras: Default::default(),
                });
            }

            // Generate animation. Spec requires animation to be nonempty.
            if !animation_channels.is_empty() {
                push_and_return_index(
                    &mut self.root.animations,
                    gltf_json::Animation {
                        name: Some("world changes".into()),
                        channels: animation_channels,
                        samplers: animation_samplers,
                        extensions: Default::default(),
                        extras: Default::default(),
                    },
                );
            }
        }

        if !scene_nodes.is_empty() {
            self.root.scenes.push(gltf_json::Scene {
                name: Some("recording".into()),
                nodes: scene_nodes,
                extras: Default::default(),
                extensions: None,
            });
        }

        Ok(self.root)
    }
}

pub(crate) async fn export_gltf(
    progress: YieldProgress,
    source: ExportSet,
    destination: PathBuf,
) -> Result<(), ExportError> {
    let ExportSet {
        contents:
            PartialUniverse {
                blocks: block_defs,
                spaces,
                characters,
            },
    } = source;

    // If unsupported list is nonempty, fail.
    if let Some(first) = spaces.get(0) {
        return Err(ExportError::NotRepresentable {
            name: Some(first.name()),
            reason: "Exporting spaces to glTF is not yet supported".into(),
        });
    }
    if let Some(first) = characters.get(0) {
        return Err(ExportError::NotRepresentable {
            name: Some(first.name()),
            reason: "Exporting characters to glTF is not yet supported".into(),
        });
    }

    let mut writer = GltfWriter::new(GltfDataDestination::new(Some(destination.clone()), 2000));
    let mesh_options = MeshOptions::new(&GraphicsOptions::default());

    for (mut p, block_def_ref) in progress.split_evenly(block_defs.len()).zip(block_defs) {
        let block_def = block_def_ref.read()?;
        let name = block_def_ref.name();
        p.set_label(&name);
        p.progress(0.01).await;
        let mesh = SpaceMesh::from(&BlockMesh::new(
            &block_def
                .evaluate()
                .map_err(|eve| ExportError::NotRepresentable {
                    name: Some(name.clone()),
                    reason: format!("block evaluation failed: {eve}"),
                })?,
            &writer.texture_allocator(),
            &mesh_options,
        ));

        let mesh_index = writer.add_mesh(&name, &mesh);
        // TODO: if the mesh is empty/None, should we include the node anyway or not?
        let mesh_node = push_and_return_index(
            &mut writer.root.nodes,
            gltf_json::Node {
                mesh: mesh_index,
                ..empty_node(Some(name.to_string()))
            },
        );

        writer.root.scenes.push(json::Scene {
            name: Some(format!("{name} display scene")),
            nodes: vec![mesh_node],
            extensions: None,
            extras: Default::default(),
        });

        p.finish().await;
    }

    {
        let file = fs::File::create(destination)?;
        writer
            .into_root(Duration::from_secs(1))?
            .to_writer_pretty(&file) // TODO: non-pretty option
            .map_err(|_| -> ExportError { todo!("serialization error conversion") })?;
        file.sync_all()?;
    }

    Ok(())
}

/// Construct gltf camera entity.
/// Note that this is not complete since it does not contain the viewpoint; a node is also needed.
fn convert_camera(name: Option<String>, camera: &Camera) -> gltf_json::Camera {
    gltf_json::Camera {
        name,
        type_: Valid(gltf_json::camera::Type::Perspective),
        orthographic: None,
        perspective: Some(gltf_json::camera::Perspective {
            aspect_ratio: Some(camera.viewport().nominal_aspect_ratio() as f32),
            yfov: camera.options().fov_y.into_inner() as f32 * (std::f32::consts::PI / 180.),
            zfar: Some(camera.options().view_distance.into_inner() as f32),
            znear: 1. / 32., // TODO: expose this from `Camera`
            extensions: Default::default(),
            extras: Default::default(),
        }),
        extensions: Default::default(),
        extras: Default::default(),
    }
}
