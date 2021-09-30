// Copyright 2020-2021 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

//! [`PixelBuf`] and output formats of the raytracer.

use std::convert::TryFrom as _;

use cgmath::{Vector3, Zero as _};

use crate::math::Rgba;
use crate::space::SpaceBlockData;

/// Implementations of [`PixelBuf`] define output formats of the raytracer, by being
/// responsible for accumulating the color (and/or other information) for each image
/// pixel.
///
/// They should be an efficiently updatable buffer able to accumulate partial values,
/// and it must represent the transparency so as to be able to signal when to stop
/// tracing.
///
/// The implementation of the [`Default`] trait must provide a suitable initial state,
/// i.e. fully transparent/no light accumulated.
pub trait PixelBuf: Default {
    /// Type of the pixel value this [`PixelBuf`] produces; the value that will be
    /// returned by tracing a single ray.
    ///
    /// This trait does not define how multiple pixels are combined into an image.
    type Pixel: Send + Sync + 'static;

    /// Type of the data precomputed for each distinct block by
    /// [`Self::compute_block_data()`].
    ///
    /// If no data beyond color is needed, this may be `()`.
    // Note: I tried letting BlockData contain references but I couldn't satisfy
    // the borrow checker.
    type BlockData: Send + Sync + 'static;

    /// Computes whatever data this [`PixelBuf`] wishes to have available in
    /// [`Self::add`], for a given block.
    fn compute_block_data(block: &SpaceBlockData) -> Self::BlockData;

    /// Computes whatever value should be passed to [`Self::add`] when the raytracer
    /// encounters an error.
    fn error_block_data() -> Self::BlockData;

    /// Computes whatever value should be passed to [`Self::add`] when the raytracer
    /// encounters the sky (background behind all blocks).
    fn sky_block_data() -> Self::BlockData;

    /// Returns whether `self` has recorded an opaque surface and therefore will not
    /// be affected by future calls to [`Self::add`].
    fn opaque(&self) -> bool;

    /// Computes the value the raytracer should return for this pixel when tracing is
    /// complete.
    fn result(self) -> Self::Pixel;

    /// Adds the color of a surface to the buffer. The provided color should already
    /// have the effect of lighting applied.
    ///
    /// You should probably give this method the `#[inline]` attribute.
    ///
    /// TODO: this interface might want even more information; generalize it to be
    /// more future-proof.
    fn add(&mut self, surface_color: Rgba, block_data: &Self::BlockData);

    /// Indicates that the trace did not intersect any space that could have contained
    /// anything to draw. May be used for special diagnostic drawing. If used, should
    /// disable the effects of future [`Self::add`] calls.
    fn hit_nothing(&mut self) {}
}

/// Implements [`PixelBuf`] for RGB(A) color with [`f32`] components.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorBuf {
    /// Color buffer.
    ///
    /// The value can be interpreted as being “premultiplied alpha” value where the alpha
    /// is `1.0 - self.ray_alpha`, or equivalently we can say that it is the color to
    /// display supposing that everything not already traced is black.
    ///
    /// Note: Not using the [`Rgb`](crate::math::Rgb) type so as to skip NaN checks.
    color_accumulator: Vector3<f32>,

    /// Fraction of the color value that is to be determined by future, rather than past,
    /// tracing; starts at 1.0 and decreases as surfaces are encountered.
    ray_alpha: f32,
}

impl PixelBuf for ColorBuf {
    type Pixel = Rgba;
    type BlockData = ();

    fn compute_block_data(_: &SpaceBlockData) {}

    fn error_block_data() {}

    fn sky_block_data() {}

    #[inline]
    fn result(self) -> Rgba {
        if self.ray_alpha >= 1.0 {
            // Special case to avoid dividing by zero
            Rgba::TRANSPARENT
        } else {
            let color_alpha = 1.0 - self.ray_alpha;
            let non_premultiplied_color = self.color_accumulator / color_alpha;
            Rgba::try_from(non_premultiplied_color.extend(color_alpha))
                .unwrap_or_else(|_| Rgba::new(1.0, 0.0, 0.0, 1.0))
        }
    }

    #[inline]
    fn opaque(&self) -> bool {
        // Let's suppose that we don't care about differences that can't be represented
        // in 8-bit color...not considering gamma.
        self.ray_alpha < 1.0 / 256.0
    }

    #[inline]
    fn add(&mut self, surface_color: Rgba, _block_data: &Self::BlockData) {
        let color_vector: Vector3<f32> = surface_color.to_rgb().into();
        let surface_alpha = surface_color.alpha().into_inner();
        let alpha_for_add = surface_alpha * self.ray_alpha;
        self.ray_alpha *= 1.0 - surface_alpha;
        self.color_accumulator += color_vector * alpha_for_add;
    }
}

impl Default for ColorBuf {
    #[inline]
    fn default() -> Self {
        Self {
            color_accumulator: Vector3::zero(),
            ray_alpha: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_buf() {
        let color_1 = Rgba::new(1.0, 0.0, 0.0, 0.75);
        let color_2 = Rgba::new(0.0, 1.0, 0.0, 0.5);
        let color_3 = Rgba::new(0.0, 0.0, 1.0, 1.0);

        let mut buf = ColorBuf::default();
        assert_eq!(buf.clone().result(), Rgba::TRANSPARENT);
        assert!(!buf.opaque());

        buf.add(color_1, &());
        assert_eq!(buf.clone().result(), color_1);
        assert!(!buf.opaque());

        buf.add(color_2, &());
        // TODO: this is not the right assertion because it's the premultiplied form.
        // assert_eq!(
        //     buf.result(),
        //     (color_1.to_rgb() * 0.75 + color_2.to_rgb() * 0.125)
        //         .with_alpha(NotNan::new(0.875).unwrap())
        // );
        assert!(!buf.opaque());

        buf.add(color_3, &());
        assert!(buf.clone().result().fully_opaque());
        //assert_eq!(
        //    buf.result(),
        //    (color_1.to_rgb() * 0.75 + color_2.to_rgb() * 0.125 + color_3.to_rgb() * 0.125)
        //        .with_alpha(NotNan::one())
        //);
        assert!(buf.opaque());
    }
}