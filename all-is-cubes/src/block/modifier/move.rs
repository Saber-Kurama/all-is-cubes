use cgmath::Zero;

use crate::block::{
    self, Block, BlockAttributes, BlockCollision, EvaluatedBlock, Evoxel, Modifier,
    Resolution::R16, AIR,
};
use crate::drawing::VoxelBrush;
use crate::math::{Face6, GridAab, GridArray, GridCoordinate, Rgba};
use crate::universe;

/// Data for [`Modifier::Move`]; displaces the block out of the grid, cropping it.
/// A pair of `Move`s can depict a block moving between two cubes.
///
/// # Animation
///
/// * If the `distance` is zero then the modifier will remove itself, if possible,
///   on the next tick.
/// * If the `distance` and `velocity` are such that the block is out of view and will
///   never strt being in view, the block will be replaced with [`AIR`].
///
/// (TODO: Define the conditions for “if possible”.)
#[non_exhaustive] // TODO: needs a constructor instead
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Move {
    /// The direction in which the block is displaced.
    pub direction: Face6,
    /// The distance, in 1/256ths, by which it is displaced.
    pub distance: u16,
    /// The velocity **per tick** with which the displacement is changing.
    ///
    /// TODO: "Per tick" is a bad unit.
    pub velocity: i16,
}

impl Move {
    // TODO: add general constructor

    /// Create a pair of [`Modifier::Move`]s to displace a block.
    /// The first goes on the block being moved and the second on the air
    /// it's moving into.
    ///
    /// TODO: This is going to need to change again in order to support
    /// moving one block in and another out at the same time.
    pub fn paired_move(direction: Face6, distance: u16, velocity: i16) -> [Modifier; 2] {
        [
            Modifier::Move(Move {
                direction,
                distance,
                velocity,
            }),
            Modifier::Move(Move {
                direction: direction.opposite(),
                distance: 256 - distance,
                velocity: -velocity,
            }),
        ]
    }

    /// Note that `Modifier::Move` does some preprocessing to keep this simpler.
    pub(super) fn evaluate(
        &self,
        block: &Block,
        this_modifier_index: usize,
        mut input: EvaluatedBlock,
        depth: u8,
    ) -> Result<EvaluatedBlock, block::EvalBlockError> {
        let Move {
            direction,
            distance,
            velocity,
        } = *self;

        // Apply Quote to ensure that the block's own `tick_action` and other effects
        // don't interfere with movement or cause duplication.
        // (In the future we may want a more nuanced policy that allows internal changes,
        // but that will probably involve refining tick_action processing.)
        input = Modifier::from(block::Quote::default()).evaluate(
            block,
            this_modifier_index,
            input,
            depth,
        )?;

        let (original_bounds, effective_resolution) = match input.voxels.as_ref() {
            Some(array) => (array.bounds(), input.resolution),
            // Treat color blocks as having a resolution of 16. TODO: Improve on this hardcoded constant
            None => (GridAab::for_block(R16), R16),
        };

        // For now, our strategy is to work in units of the block's resolution.
        // TODO: Generalize to being able to increase resolution to a chosen minimum.
        let distance_in_res =
            GridCoordinate::from(distance) * GridCoordinate::from(effective_resolution) / 256;
        let translation_in_res = direction.normal_vector() * distance_in_res;

        // This will be None if the displacement puts the block entirely out of view.
        let displaced_bounds: Option<GridAab> = original_bounds
            .translate(translation_in_res)
            .intersection(GridAab::for_block(effective_resolution));

        let animation_action = if displaced_bounds.is_none() && velocity >= 0 {
            // Displaced to invisibility; turn into just plain air.
            Some(VoxelBrush::single(AIR))
        } else if translation_in_res.is_zero() && velocity == 0 || distance == 0 && velocity < 0 {
            // Either a stationary displacement which is invisible, or an animated one which has finished its work.
            assert!(
                matches!(&block.modifiers()[this_modifier_index], Modifier::Move(m) if m == self)
            );
            let mut new_block = block.clone();
            new_block.modifiers_mut().remove(this_modifier_index); // TODO: What if other modifiers want to do things?
            Some(VoxelBrush::single(new_block))
        } else if velocity != 0 {
            // Movement in progress.
            assert!(
                matches!(&block.modifiers()[this_modifier_index], Modifier::Move(m) if m == self)
            );
            let mut new_block = block.clone();
            if let Modifier::Move(Move {
                distance, velocity, ..
            }) = &mut new_block.modifiers_mut()[this_modifier_index]
            {
                *distance = i32::from(*distance)
                            .saturating_add(i32::from(*velocity))
                            .clamp(0, i32::from(u16::MAX))
                            .try_into()
                            .unwrap(/* clamped to range */);
            }
            Some(VoxelBrush::single(new_block))
        } else {
            // Stationary displacement; take no action
            None
        };

        // Used by the solid color case; we have to do this before we move `attributes`
        // out of `value`.
        let voxel = Evoxel::from_block(&input);

        let attributes = BlockAttributes {
            // Switch to `Recur` collision so that the displacement collides as expected.
            // TODO: If the collision was `Hard` then we may need to edit the collision
            // values of the individual voxels to preserve expected behavior.
            collision: match input.attributes.collision {
                BlockCollision::None => BlockCollision::None,
                BlockCollision::Hard | BlockCollision::Recur => {
                    if displaced_bounds.is_some() {
                        BlockCollision::Recur
                    } else {
                        // Recur treats no-voxels as Hard, which is not what we want
                        BlockCollision::None
                    }
                }
            },
            tick_action: animation_action,
            ..input.attributes
        };

        Ok(match displaced_bounds {
            Some(displaced_bounds) => {
                let displaced_voxels = match input.voxels.as_ref() {
                    Some(voxels) => GridArray::from_fn(displaced_bounds, |cube| {
                        voxels[cube - translation_in_res]
                    }),
                    None => {
                        // Input block is a solid color; synthesize voxels.
                        GridArray::from_fn(displaced_bounds, |_| voxel)
                    }
                };
                EvaluatedBlock::from_voxels(attributes, effective_resolution, displaced_voxels)
            }
            None => EvaluatedBlock::from_color(attributes, Rgba::TRANSPARENT),
        })
    }
}

impl From<Move> for block::Modifier {
    fn from(value: Move) -> Self {
        Modifier::Move(value)
    }
}

impl universe::VisitRefs for Move {
    fn visit_refs(&self, _visitor: &mut dyn universe::RefVisitor) {
        let Move {
            direction: _,
            distance: _,
            velocity: _,
        } = self;
    }
}

#[cfg(test)]
mod tests {
    use cgmath::EuclideanSpace;

    use crate::block::{Block, Modifier, Resolution::R2};
    use crate::content::make_some_blocks;
    use crate::math::{GridPoint, OpacityCategory};
    use crate::space::Space;
    use crate::time::Tick;
    use crate::universe::Universe;

    use super::*;

    #[test]
    fn move_atom_block_evaluation() {
        let color = rgba_const!(1.0, 0.0, 0.0, 1.0);
        let original = Block::from(color);
        let modifier = Modifier::from(Move {
            direction: Face6::PY,
            distance: 128, // distance 1/2 block × scale factor of 256
            velocity: 0,
        });
        let moved = modifier.attach(original.clone());

        let expected_bounds = GridAab::from_lower_size([0, 8, 0], [16, 8, 16]);

        let ev_original = original.evaluate().unwrap();
        assert_eq!(
            moved.evaluate().unwrap(),
            EvaluatedBlock {
                attributes: BlockAttributes {
                    collision: BlockCollision::Recur,
                    ..ev_original.attributes.clone()
                },
                color: color.to_rgb().with_alpha(notnan!(0.5)),
                voxels: Some(GridArray::repeat(
                    expected_bounds,
                    Evoxel::from_block(&ev_original)
                )),
                resolution: R16,
                opaque: false,
                visible: true,
                voxel_opacity_mask: Some(GridArray::repeat(
                    expected_bounds,
                    OpacityCategory::Opaque
                )),
            }
        );
    }

    #[test]
    fn move_voxel_block_evaluation() {
        let mut universe = Universe::new();
        let resolution = R2;
        let color = rgba_const!(1.0, 0.0, 0.0, 1.0);
        let original = Block::builder()
            .voxels_fn(&mut universe, resolution, |_| Block::from(color))
            .unwrap()
            .build();

        let modifier = Modifier::from(Move {
            direction: Face6::PY,
            distance: 128, // distance 1/2 block × scale factor of 256
            velocity: 0,
        });
        let moved = modifier.attach(original.clone());

        let expected_bounds = GridAab::from_lower_size([0, 1, 0], [2, 1, 2]);

        let ev_original = original.evaluate().unwrap();
        assert_eq!(
            moved.evaluate().unwrap(),
            EvaluatedBlock {
                attributes: BlockAttributes {
                    collision: BlockCollision::Recur,
                    ..ev_original.attributes.clone()
                },
                color: color.to_rgb().with_alpha(notnan!(0.5)),
                voxels: Some(GridArray::repeat(
                    expected_bounds,
                    Evoxel::from_block(&ev_original)
                )),
                resolution,
                opaque: false,
                visible: true,
                voxel_opacity_mask: Some(GridArray::repeat(
                    expected_bounds,
                    OpacityCategory::Opaque
                )),
            }
        );
    }

    /// [`Modifier::Move`] incorporates [`Modifier::Quote`] to ensure that no conflicting
    /// effects happen.
    #[test]
    fn move_also_quotes() {
        let original = Block::builder()
            .color(Rgba::WHITE)
            .tick_action(Some(VoxelBrush::single(AIR)))
            .build();
        let moved = Modifier::from(Move {
            direction: Face6::PY,
            distance: 128,
            velocity: 0,
        })
        .attach(original);

        assert_eq!(moved.evaluate().unwrap().attributes.tick_action, None);
    }

    /// Set up a `Modifier::Move`, let it run, and then allow assertions to be made about the result.
    fn move_block_test(direction: Face6, velocity: i16, checker: impl FnOnce(&Space, &Block)) {
        let [block] = make_some_blocks();
        let mut space = Space::empty(GridAab::from_lower_upper([-1, -1, -1], [2, 2, 2]));
        let [move_out, move_in] = Move::paired_move(direction, 0, velocity);
        space
            .set([0, 0, 0], move_out.attach(block.clone()))
            .unwrap();
        space
            .set(
                GridPoint::origin() + direction.normal_vector(),
                move_in.attach(block.clone()),
            )
            .unwrap();
        let mut universe = Universe::new();
        let space = universe.insert_anonymous(space);
        // TODO: We need a "step until idle" function, or for the UniverseStepInfo to convey how many blocks were updated / are waiting
        // TODO: Some tests will want to look at the partial results
        for _ in 0..257 {
            universe.step(Tick::arbitrary());
        }
        checker(&space.borrow(), &block);
    }

    #[test]
    fn velocity_zero() {
        move_block_test(Face6::PX, 0, |space, block| {
            assert_eq!(&space[[0, 0, 0]], block);
            assert_eq!(&space[[1, 0, 0]], &AIR);
        });
    }

    #[test]
    fn velocity_slow() {
        move_block_test(Face6::PX, 1, |space, block| {
            assert_eq!(&space[[0, 0, 0]], &AIR);
            assert_eq!(&space[[1, 0, 0]], block);
        });
    }

    #[test]
    fn velocity_whole_cube_in_one_tick() {
        move_block_test(Face6::PX, 256, |space, block| {
            assert_eq!(&space[[0, 0, 0]], &AIR);
            assert_eq!(&space[[1, 0, 0]], block);
        });
    }
}