// Copyright 2020-2022 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

use crate::block::{Block, BlockChange, EvalBlockError, EvaluatedBlock};
use crate::listen::Listener;
use crate::math::{GridRotation, Rgb};
use crate::space::GridArray;
use crate::universe::{RefVisitor, VisitRefs};

/// Modifiers can be applied to a [`Block`] to change the result of
/// [`evaluate()`](Block::evaluate)ing it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Modifier {
    /// Suppresses all behaviors of the [`Block`] that might affect the space around it,
    /// (or itself).
    // TODO: figure out how to publicly construct this given that we want to still have
    // the option to add variants
    #[non_exhaustive]
    Quote {
        /// If true, also suppress light and sound effects.
        ambient: bool,
    },

    /// Rotate the block about its cube center by the given rotation.
    Rotate(GridRotation),
}

impl Modifier {
    /// Return the given `block` with `self` added as the outermost modifier.
    pub fn attach(self, mut block: Block) -> Block {
        block.modifiers_mut().push(self);
        block
    }

    pub(crate) fn apply(
        &self,
        mut value: EvaluatedBlock,
        _depth: u8,
    ) -> Result<EvaluatedBlock, EvalBlockError> {
        Ok(match *self {
            Modifier::Quote { ambient } => {
                value.attributes.tick_action = None;
                if ambient {
                    value.attributes.light_emission = Rgb::ZERO;
                }
                value
            }

            Modifier::Rotate(rotation) => {
                if value.voxels.is_none() && value.voxel_opacity_mask.is_none() {
                    // Skip computation of transforms
                    value
                } else {
                    // TODO: Add a shuffle-in-place rotation operation to GridArray and try implementing this using that, which should have less arithmetic involved than these matrix ops
                    let resolution = value.resolution;
                    let inner_to_outer = rotation.to_positive_octant_matrix(resolution.into());
                    let outer_to_inner = rotation
                        .inverse()
                        .to_positive_octant_matrix(resolution.into());

                    EvaluatedBlock {
                        voxels: value.voxels.map(|voxels| {
                            GridArray::from_fn(
                                voxels.grid().transform(inner_to_outer).unwrap(),
                                |cube| voxels[outer_to_inner.transform_cube(cube)],
                            )
                        }),
                        voxel_opacity_mask: value.voxel_opacity_mask.map(|mask| {
                            GridArray::from_fn(
                                mask.grid().transform(inner_to_outer).unwrap(),
                                |cube| mask[outer_to_inner.transform_cube(cube)],
                            )
                        }),

                        // Unaffected
                        attributes: value.attributes,
                        color: value.color,
                        resolution,
                        opaque: value.opaque,
                        visible: value.visible,
                    }
                }
            }
        })
    }

    /// Called by [`Block::listen()`]; not designed to be used otherwise.
    pub(crate) fn listen_impl(
        &self,
        _listener: &(impl Listener<BlockChange> + Clone + Send + Sync),
        _depth: u8,
    ) -> Result<(), EvalBlockError> {
        match self {
            Modifier::Quote { .. } => {}
            Modifier::Rotate(_) => {}
        }
        Ok(())
    }
}

impl VisitRefs for Modifier {
    fn visit_refs(&self, _visitor: &mut dyn RefVisitor) {
        match self {
            Modifier::Quote { .. } => {}
            Modifier::Rotate(..) => {}
        }
    }
}
