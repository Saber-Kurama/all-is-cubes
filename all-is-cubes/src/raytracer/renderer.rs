// Copyright 2020-2022 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

use std::fmt;

use futures_core::future::BoxFuture;
use image::RgbaImage;

use crate::apps::StandardCameras;
use crate::camera::{HeadlessRenderer, RenderError};
use crate::character::Cursor;
use crate::listen::ListenableSource;
use crate::math::Rgba;
use crate::raytracer::{ColorBuf, RtBlockData, UpdatingSpaceRaytracer};

/// Builds upon [`UpdatingSpaceRaytracer`] to make a complete [`HeadlessRenderer`],
/// following the scene and camera information in a [`StandardCameras`].
pub struct RtRenderer<D: RtBlockData = ()> {
    cameras: StandardCameras,
    rt: UpdatingSpaceRaytracer<D>,
}

impl RtRenderer {
    pub fn new(cameras: StandardCameras) -> Self {
        let rt = UpdatingSpaceRaytracer::<()>::new(
            // TODO: We need to follow the cameras' character instead of snapshotting here
            cameras
                .world_space()
                .snapshot()
                .expect("No world space given!"),
            // TODO: StandardCameras should expose the options source
            ListenableSource::constant(cameras.graphics_options().clone()),
            ListenableSource::constant(()),
        );
        RtRenderer { cameras, rt }
    }
}

// manual impl avoids `D: Debug` bound
impl<D: RtBlockData> fmt::Debug for RtRenderer<D>
where
    D::Options: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtRenderer")
            .field("cameras", &self.cameras)
            .field("rt", &self.rt)
            .finish()
    }
}

impl HeadlessRenderer for RtRenderer<()> {
    fn update<'a>(
        &'a mut self,
        _cursor: Option<&'a Cursor>,
    ) -> BoxFuture<'a, Result<(), RenderError>> {
        Box::pin(async {
            // TODO: raytracer needs to implement drawing the cursor
            self.rt.update().map_err(RenderError::Read)?;
            Ok(())
        })
    }

    fn draw<'a>(
        &'a mut self,
        _info_text: &'a str,
    ) -> BoxFuture<'a, Result<RgbaImage, RenderError>> {
        // TODO: implement drawing info text (can use embedded_graphics for that)
        Box::pin(async {
            let RtRenderer { cameras, rt } = self;
            let camera = cameras.cameras().world.clone();
            let (image, _info) = rt
                .get()
                .trace_scene_to_image::<ColorBuf, _, Rgba>(&camera, |pixel_buf| {
                    camera.post_process_color(Rgba::from(pixel_buf))
                });

            let image = RgbaImage::from_raw(
                camera.viewport().framebuffer_size.x,
                camera.viewport().framebuffer_size.y,
                Vec::from(image)
                    .into_iter()
                    .flat_map(|color| color.to_srgb8())
                    .collect::<Vec<u8>>(),
            )
            .unwrap(/* can't happen: wrong dimensions */);

            Ok(image)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _renderer_is_send_sync()
    where
        RtRenderer: Send + Sync + 'static,
    {
    }
}
