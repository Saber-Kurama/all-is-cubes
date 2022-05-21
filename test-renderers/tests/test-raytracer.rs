// Copyright 2020-2022 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

use all_is_cubes::apps::StandardCameras;
use all_is_cubes::camera::HeadlessRenderer;
use all_is_cubes::raytracer::RtRenderer;
use test_renderers::{RendererFactory, RendererId};

#[allow(clippy::result_unit_err)]
#[cfg(test)]
#[tokio::main]
pub async fn main() -> Result<(), ()> {
    test_renderers::harness_main(
        RendererId::Raytracer,
        test_renderers::test_cases::all_tests,
        || std::future::ready(RtFactory),
    )
    .await
}

#[derive(Clone, Debug)]
struct RtFactory;

impl RendererFactory for RtFactory {
    fn renderer_from_cameras(&self, cameras: StandardCameras) -> Box<dyn HeadlessRenderer + Send> {
        Box::new(RtRenderer::new(cameras))
    }

    fn id(&self) -> RendererId {
        RendererId::Raytracer
    }
}
