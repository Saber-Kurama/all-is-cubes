// Copyright 2020-2021 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

//! Voxel User Interface.
//!
//! We've got all this rendering and interaction code, so let's reuse it for the
//! GUI as well as the game.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

use cgmath::{Angle as _, Deg, Matrix4, Vector3};
use embedded_graphics::geometry::Point;
use embedded_graphics::prelude::{Drawable, Primitive};
use embedded_graphics::primitives::{PrimitiveStyleBuilder, Rectangle};
use ordered_float::NotNan;

use crate::apps::{InputProcessor, Tick};
use crate::block::Block;
use crate::camera::{FogOption, GraphicsOptions};
use crate::character::Character;
use crate::content::palette;
use crate::drawing::VoxelBrush;
use crate::listen::{ListenableSource, Listener};
use crate::math::{FreeCoordinate, GridMatrix};
use crate::space::Space;

use crate::universe::{URef, Universe, UniverseStepInfo};

mod hud;
use hud::*;
mod icons;
pub use icons::*;
mod widget;
pub(crate) use widget::*;

/// `Vui` builds user interfaces out of voxels. It owns a `Universe` dedicated to the
/// purpose and draws into spaces to form the HUD and menus.
#[derive(Debug)] // TODO: probably not very informative Debug as derived
pub(crate) struct Vui {
    universe: Universe,
    current_space: URef<Space>,

    hud_blocks: HudBlocks,
    hud_space: URef<Space>,
    hud_layout: HudLayout,
    hud_widgets: Vec<Box<dyn WidgetController>>,
    aspect_ratio: FreeCoordinate,

    tooltip_state: Arc<Mutex<TooltipState>>,

    todo: Rc<RefCell<VuiTodo>>,

    // Things we're listening to...
    mouselook_mode: ListenableSource<bool>,
    paused: ListenableSource<bool>,
}

impl Vui {
    /// `input_processor` is the `InputProcessor` whose state may be reflected on the HUD.
    /// `character` is the `Character` whose inventory is displayed. TODO: Allow for character switching
    /// TODO: Reduce coupling, perhaps by passing in a separate struct with just the listenable
    /// elements.
    pub fn new(
        input_processor: &InputProcessor,
        paused: ListenableSource<bool>,
        character: Option<URef<Character>>,
    ) -> Self {
        let mut universe = Universe::new();
        let hud_blocks = HudBlocks::new(&mut universe, 16);
        let hud_layout = HudLayout::default();
        let hud_space = hud_layout.new_space(&mut universe, &hud_blocks);

        let todo = Rc::new(RefCell::new(VuiTodo::default()));
        let tooltip_state = Arc::default();
        if let Some(character_ref) = &character {
            TooltipState::bind_to_character(&tooltip_state, character_ref.clone());
        }

        // TODO: HudLayout should take care of this maybe
        let hud_widgets: Vec<Box<dyn WidgetController>> = vec![
            Box::new(ToolbarController::new(character, &hud_layout)),
            Box::new(CrosshairController::new(
                hud_layout.crosshair_position(),
                input_processor.mouselook_mode(),
            )),
            Box::new(TooltipController::new(
                Arc::clone(&tooltip_state),
                &mut hud_space.borrow_mut(),
                &hud_layout,
                &mut universe,
            )),
        ];

        Self {
            universe,
            current_space: hud_space.clone(),
            hud_blocks,
            hud_space,
            hud_layout,
            hud_widgets,
            aspect_ratio: 4. / 3., // arbitrary placeholder assumption

            tooltip_state,

            todo,

            mouselook_mode: input_processor.mouselook_mode(),
            paused,
        }
    }

    // TODO: It'd be more encapsulating if we could provide a _read-only_ reference...
    pub fn current_space(&self) -> &URef<Space> {
        &self.current_space
    }

    /// Computes an OpenGL style view matrix that should be used to display the
    /// [`Vui::current_space`].
    ///
    /// It does not need to be rechecked other than on aspect ratio changes.
    ///
    /// TODO: This is not a method because the code structure makes it inconvenient for
    /// renderers to get access to `Vui` itself. Add some other communication path.
    pub fn view_matrix(space: &Space, fov_y: Deg<FreeCoordinate>) -> Matrix4<FreeCoordinate> {
        let grid = space.grid();
        let mut ui_center = grid.center();

        // Arrange a view distance which will place the Z=0 plane sized to fill the viewport
        // (at least vertically, as we don't have aspect ratio support yet).
        ui_center.z = 0.0;

        let view_distance = FreeCoordinate::from(grid.size().y) * (fov_y / 2.).cot() / 2.;
        Matrix4::look_at_rh(
            ui_center + Vector3::new(0., 0., view_distance),
            ui_center,
            Vector3::new(0., 1., 0.),
        )
    }

    /// Compute graphics options to render the VUI space given the user's regular options.
    pub fn graphics_options(mut options: GraphicsOptions) -> GraphicsOptions {
        // Set FOV to give a predictable, not-too-wide-angle perspective.
        options.fov_y = NotNan::new(30.).unwrap();

        // Disable fog for maximum clarity and because we shouldn't have any far clipping to hide.
        options.fog = FogOption::None;

        // Fixed view distance for our layout.
        // TODO: Derive this from HudLayout and also FOV (since FOV determines eye-to-space distance).
        options.view_distance = NotNan::new(100.0).unwrap();

        // clutter
        options.debug_chunk_boxes = false;

        options
    }

    pub fn step(&mut self, tick: Tick) -> UniverseStepInfo {
        let sv = WidgetSpaceView {
            hud_blocks: &self.hud_blocks,
            space: self.hud_space.clone(),
        };
        for controller in &mut self.hud_widgets {
            if let Err(e) = controller.step(&sv, tick) {
                // TODO: reduce log-spam if this ever happens
                log::error!("VUI widget error: {}\nSource:{:#?}", e, controller);
            }
        }

        self.universe.step(tick)
    }
}

/// [`Vui`]'s set of things that need updating.
#[derive(Debug, Default)]
struct VuiTodo {}

/// [`Listener`] adapter for [`VuiTodo`].
struct TodoListener {
    target: Weak<RefCell<VuiTodo>>,
    handler: fn(&mut VuiTodo),
}

impl Listener<()> for TodoListener {
    fn receive(&self, _message: ()) {
        if let Some(cell) = self.target.upgrade() {
            let mut todo = RefCell::borrow_mut(&cell);
            (self.handler)(&mut todo);
        }
    }

    fn alive(&self) -> bool {
        self.target.strong_count() > 0
    }
}

#[allow(unused)] // TODO: not yet used for real
pub(crate) fn draw_background(space: &mut Space) {
    let grid = space.grid();
    let background_rect = Rectangle::with_corners(
        Point::new(grid.lower_bounds().x, grid.lower_bounds().y),
        Point::new(grid.upper_bounds().x - 1, grid.upper_bounds().y - 1),
    );

    let display =
        &mut space.draw_target(GridMatrix::from_translation([0, 0, grid.lower_bounds().z]));

    let background = VoxelBrush::single(Block::from(palette::MENU_BACK));
    let frame = VoxelBrush::single(Block::from(palette::MENU_FRAME)).translate((0, 0, 1));

    background_rect
        .into_styled(
            PrimitiveStyleBuilder::new()
                .stroke_width(1)
                .stroke_color(&frame)
                .fill_color(&background)
                .build(),
        )
        .draw(display)
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_vui_for_test() -> Vui {
        Vui::new(
            &InputProcessor::new(),
            ListenableSource::constant(false),
            None,
        )
    }

    #[test]
    fn vui_smoke_test() {
        let _ = new_vui_for_test();
    }

    #[test]
    fn background_smoke_test() {
        let mut space = Space::empty_positive(100, 100, 10);
        draw_background(&mut space);
    }
}
