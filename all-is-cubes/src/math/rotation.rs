// Copyright 2020-2021 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

//! Rotations which exchange axes (thus not leaving the integer grid).
//! This module is private but reexported by its parent.

use std::ops::Mul;

use cgmath::{One, Vector3, Zero as _};

use crate::math::*;

/// Represents a discrete (grid-aligned) rotation, or exchange of axes.
///
/// Compared to a [`GridMatrix`], this cannot specify scale, translation, or skew;
/// it is used for identifying the rotations of blocks.
///
/// Each of the variant names specifies the three unit vectors which (*x*, *y*, *z*),
/// respectively, should be multiplied by to perform the rotation.
/// Lowercase refers to a negated unit vector.
///
/// See also:
///
/// * [`Face`] is less general, in that it specifies a single axis but not
///   rotation about that axis.
/// * [`GridMatrix`] is more general, specifying an affine transformation.
#[rustfmt::skip]
#[allow(clippy::upper_case_acronyms)]
#[allow(clippy::exhaustive_enums)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum GridRotation {
    // TODO: shuffle or explicitly number these to choose a meaningful numbering
    RXYZ, RXYz, RXyZ, RXyz, RxYZ, RxYz, RxyZ, Rxyz,
    RXZY, RXZy, RXzY, RXzy, RxZY, RxZy, RxzY, Rxzy,
    RYXZ, RYXz, RYxZ, RYxz, RyXZ, RyXz, RyxZ, Ryxz,
    RYZX, RYZx, RYzX, RYzx, RyZX, RyZx, RyzX, Ryzx,
    RZXY, RZXy, RZxY, RZxy, RzXY, RzXy, RzxY, Rzxy,
    RZYX, RZYx, RZyX, RZyx, RzYX, RzYx, RzyX, Rzyx,
}

impl GridRotation {
    /// All 48 possible rotations.
    ///
    /// Warning: TODO: The ordering of these rotations is not yet stable.
    /// The current ordering is based on the six axis permutations followed by rotations.
    #[rustfmt::skip]
    pub const ALL: [Self; 48] = {
        use GridRotation::*;
        [
            RXYZ, RXYz, RXyZ, RXyz, RxYZ, RxYz, RxyZ, Rxyz,
            RXZY, RXZy, RXzY, RXzy, RxZY, RxZy, RxzY, Rxzy,
            RYXZ, RYXz, RYxZ, RYxz, RyXZ, RyXz, RyxZ, Ryxz,
            RYZX, RYZx, RYzX, RYzx, RyZX, RyZx, RyzX, Ryzx,
            RZXY, RZXy, RZxY, RZxy, RzXY, RzXy, RzxY, Rzxy,
            RZYX, RZYx, RZyX, RZyx, RzYX, RzYx, RzyX, Rzyx,
        ]
    };

    pub const IDENTITY: Self = Self::RXYZ;

    /// The rotation that is clockwise in our Y-up right-handed coordinate system.
    ///
    /// ```
    /// use all_is_cubes::math::{Face::*, GridRotation};
    ///
    /// assert_eq!(GridRotation::CLOCKWISE.transform(PX), PZ);
    /// assert_eq!(GridRotation::CLOCKWISE.transform(PZ), NX);
    /// assert_eq!(GridRotation::CLOCKWISE.transform(NX), NZ);
    /// assert_eq!(GridRotation::CLOCKWISE.transform(NZ), PX);
    ///
    /// assert_eq!(GridRotation::CLOCKWISE.transform(PY), PY);
    /// ```
    pub const CLOCKWISE: Self = Self::RZYx;

    /// The rotation that is counterclockwise in our Y-up right-handed coordinate system.
    ///
    /// ```
    /// use all_is_cubes::math::{Face::*, GridRotation};
    ///
    /// assert_eq!(GridRotation::COUNTERCLOCKWISE.transform(PX), NZ);
    /// assert_eq!(GridRotation::COUNTERCLOCKWISE.transform(NZ), NX);
    /// assert_eq!(GridRotation::COUNTERCLOCKWISE.transform(NX), PZ);
    /// assert_eq!(GridRotation::COUNTERCLOCKWISE.transform(PZ), PX);
    ///
    /// assert_eq!(GridRotation::COUNTERCLOCKWISE.transform(PY), PY);
    /// ```
    pub const COUNTERCLOCKWISE: Self = Self::RzYX;

    /// Constructs a rotation from a basis: that is, the returned rotation will
    /// rotate `PX` into `basis[0]`, `PY` into `basis[1]`, and `PZ` into `basis[2]`.
    ///
    /// Panics if the three provided axes are not mutually perpendicular.
    #[inline]
    pub fn from_basis(basis: impl Into<Vector3<Face>>) -> Self {
        let basis: Vector3<Face> = basis.into();
        let basis: [Face; 3] = basis.into(); // for concise matching
        use {Face::*, GridRotation::*};
        match basis {
            [PX, PY, PZ] => RXYZ,
            [PX, PZ, PY] => RXZY,
            [PY, PX, PZ] => RYXZ,
            [PY, PZ, PX] => RYZX,
            [PZ, PX, PY] => RZXY,
            [PZ, PY, PX] => RZYX,

            [PX, PY, NZ] => RXYz,
            [PX, PZ, NY] => RXZy,
            [PY, PX, NZ] => RYXz,
            [PY, PZ, NX] => RYZx,
            [PZ, PX, NY] => RZXy,
            [PZ, PY, NX] => RZYx,

            [PX, NY, PZ] => RXyZ,
            [PX, NZ, PY] => RXzY,
            [PY, NX, PZ] => RYxZ,
            [PY, NZ, PX] => RYzX,
            [PZ, NX, PY] => RZxY,
            [PZ, NY, PX] => RZyX,

            [PX, NY, NZ] => RXyz,
            [PX, NZ, NY] => RXzy,
            [PY, NX, NZ] => RYxz,
            [PY, NZ, NX] => RYzx,
            [PZ, NX, NY] => RZxy,
            [PZ, NY, NX] => RZyx,

            [NX, PY, PZ] => RxYZ,
            [NX, PZ, PY] => RxZY,
            [NY, PX, PZ] => RyXZ,
            [NY, PZ, PX] => RyZX,
            [NZ, PX, PY] => RzXY,
            [NZ, PY, PX] => RzYX,

            [NX, PY, NZ] => RxYz,
            [NX, PZ, NY] => RxZy,
            [NY, PX, NZ] => RyXz,
            [NY, PZ, NX] => RyZx,
            [NZ, PX, NY] => RzXy,
            [NZ, PY, NX] => RzYx,

            [NX, NY, PZ] => RxyZ,
            [NX, NZ, PY] => RxzY,
            [NY, NX, PZ] => RyxZ,
            [NY, NZ, PX] => RyzX,
            [NZ, NX, PY] => RzxY,
            [NZ, NY, PX] => RzyX,

            [NX, NY, NZ] => Rxyz,
            [NX, NZ, NY] => Rxzy,
            [NY, NX, NZ] => Ryxz,
            [NY, NZ, NX] => Ryzx,
            [NZ, NX, NY] => Rzxy,
            [NZ, NY, NX] => Rzyx,

            _ => panic!(
                "Invalid basis given to GridRotation::from_basis: {:?}",
                basis
            ),
        }
    }

    // TODO: public? do we want this to be our API? should this also be a From impl?
    #[inline]
    #[rustfmt::skip] // dense data layout
    pub(crate) const fn to_basis(self) -> Vector3<Face> {
        use {Face::*, GridRotation::*};
        match self {
            RXYZ => Vector3 { x: PX, y: PY, z: PZ },
            RXZY => Vector3 { x: PX, y: PZ, z: PY },
            RYXZ => Vector3 { x: PY, y: PX, z: PZ },
            RYZX => Vector3 { x: PY, y: PZ, z: PX },
            RZXY => Vector3 { x: PZ, y: PX, z: PY },
            RZYX => Vector3 { x: PZ, y: PY, z: PX },

            RXYz => Vector3 { x: PX, y: PY, z: NZ },
            RXZy => Vector3 { x: PX, y: PZ, z: NY },
            RYXz => Vector3 { x: PY, y: PX, z: NZ },
            RYZx => Vector3 { x: PY, y: PZ, z: NX },
            RZXy => Vector3 { x: PZ, y: PX, z: NY },
            RZYx => Vector3 { x: PZ, y: PY, z: NX },

            RXyZ => Vector3 { x: PX, y: NY, z: PZ },
            RXzY => Vector3 { x: PX, y: NZ, z: PY },
            RYxZ => Vector3 { x: PY, y: NX, z: PZ },
            RYzX => Vector3 { x: PY, y: NZ, z: PX },
            RZxY => Vector3 { x: PZ, y: NX, z: PY },
            RZyX => Vector3 { x: PZ, y: NY, z: PX },

            RXyz => Vector3 { x: PX, y: NY, z: NZ },
            RXzy => Vector3 { x: PX, y: NZ, z: NY },
            RYxz => Vector3 { x: PY, y: NX, z: NZ },
            RYzx => Vector3 { x: PY, y: NZ, z: NX },
            RZxy => Vector3 { x: PZ, y: NX, z: NY },
            RZyx => Vector3 { x: PZ, y: NY, z: NX },

            RxYZ => Vector3 { x: NX, y: PY, z: PZ },
            RxZY => Vector3 { x: NX, y: PZ, z: PY },
            RyXZ => Vector3 { x: NY, y: PX, z: PZ },
            RyZX => Vector3 { x: NY, y: PZ, z: PX },
            RzXY => Vector3 { x: NZ, y: PX, z: PY },
            RzYX => Vector3 { x: NZ, y: PY, z: PX },

            RxYz => Vector3 { x: NX, y: PY, z: NZ },
            RxZy => Vector3 { x: NX, y: PZ, z: NY },
            RyXz => Vector3 { x: NY, y: PX, z: NZ },
            RyZx => Vector3 { x: NY, y: PZ, z: NX },
            RzXy => Vector3 { x: NZ, y: PX, z: NY },
            RzYx => Vector3 { x: NZ, y: PY, z: NX },

            RxyZ => Vector3 { x: NX, y: NY, z: PZ },
            RxzY => Vector3 { x: NX, y: NZ, z: PY },
            RyxZ => Vector3 { x: NY, y: NX, z: PZ },
            RyzX => Vector3 { x: NY, y: NZ, z: PX },
            RzxY => Vector3 { x: NZ, y: NX, z: PY },
            RzyX => Vector3 { x: NZ, y: NY, z: PX },

            Rxyz => Vector3 { x: NX, y: NY, z: NZ },
            Rxzy => Vector3 { x: NX, y: NZ, z: NY },
            Ryxz => Vector3 { x: NY, y: NX, z: NZ },
            Ryzx => Vector3 { x: NY, y: NZ, z: NX },
            Rzxy => Vector3 { x: NZ, y: NX, z: NY },
            Rzyx => Vector3 { x: NZ, y: NY, z: NX },
        }
    }

    /// Expresses this rotation as a matrix which rotates “in place” the
    /// points within the volume defined by coordinates in the range [0, size].
    ///
    /// That is, a `Grid` of that volume will be unchanged by rotation:
    ///
    /// ```
    /// use all_is_cubes::{math::GridRotation, space::Grid};
    ///
    /// let grid = Grid::for_block(8);
    /// let rotation = GridRotation::CLOCKWISE.to_positive_octant_matrix(8);
    /// assert_eq!(grid.transform(rotation), Some(grid));
    /// ```
    ///
    /// Such matrices are suitable for rotating the voxels of a block, provided
    /// that the coordinates are then transformed with [`GridMatrix::transform_cube`],
    /// *not* [`GridMatrix::transform_point`] (due to the lower-corner format of cube
    /// coordinates).
    /// ```
    /// # use all_is_cubes::{math::{GridPoint, GridRotation}, space::Grid};
    /// let rotation = GridRotation::CLOCKWISE.to_positive_octant_matrix(4);
    /// assert_eq!(rotation.transform_cube(GridPoint::new(0, 0, 0)), GridPoint::new(3, 0, 0));
    /// assert_eq!(rotation.transform_cube(GridPoint::new(3, 0, 0)), GridPoint::new(3, 0, 3));
    /// assert_eq!(rotation.transform_cube(GridPoint::new(3, 0, 3)), GridPoint::new(0, 0, 3));
    /// assert_eq!(rotation.transform_cube(GridPoint::new(0, 0, 3)), GridPoint::new(0, 0, 0));
    /// ```
    ///
    // TODO: add tests
    pub fn to_positive_octant_matrix(self, size: GridCoordinate) -> GridMatrix {
        fn offset(face: Face, size: GridCoordinate) -> GridVector {
            if face.is_positive() {
                GridVector::zero()
            } else {
                face.normal_vector() * -size
            }
        }
        let basis = self.to_basis();
        GridMatrix {
            x: basis.x.normal_vector(),
            y: basis.y.normal_vector(),
            z: basis.z.normal_vector(),
            w: offset(basis.x, size) + offset(basis.y, size) + offset(basis.z, size),
        }
    }

    /// Expresses this rotation as a matrix without any translation.
    // TODO: add tests
    pub fn to_rotation_matrix(self) -> GridMatrix {
        self.to_positive_octant_matrix(0)
    }

    // TODO: test equivalence with matrix
    #[inline]
    pub fn transform(self, face: Face) -> Face {
        // TODO: there ought to be a much cleaner way to express this
        // ... and it should be a const fn, too
        if face == Face::Within {
            face
        } else {
            let p = self.to_basis()[face.axis_number()];
            if face.is_negative() {
                p.opposite()
            } else {
                p
            }
        }
    }

    /// Returns the inverse of this rotation; the one which undoes this.
    ///
    /// ```
    /// use all_is_cubes::math::GridRotation;
    ///
    /// for &rotation in &GridRotation::ALL {
    ///     assert_eq!(rotation * rotation.inverse(), GridRotation::IDENTITY);
    /// }
    /// ```
    pub fn inverse(self) -> Self {
        // TODO: Make this more efficient. Can we do it without writing out another 48-element match?
        self.iterate().last().unwrap()
    }

    /// Generates the sequence of rotations that may be obtained by concatenating/multiplying
    /// this rotation with itself repeatedly.
    ///
    /// The first element of the iterator will always be the identity, i.e. this rotation
    /// applied zero times. The iterator ends when the sequence would repeat itself, i.e.
    /// just before it would produce the identity again.
    ///
    /// ```
    /// use all_is_cubes::math::Face::*;
    /// use all_is_cubes::math::GridRotation;
    ///
    /// assert_eq!(
    ///     GridRotation::IDENTITY.iterate().collect::<Vec<_>>(),
    ///     vec![GridRotation::IDENTITY],
    /// );
    ///
    /// let x_reflection = GridRotation::from_basis([NX, PY, PZ]);
    /// assert_eq!(
    ///     x_reflection.iterate().collect::<Vec<_>>(),
    ///     vec![GridRotation::IDENTITY, x_reflection],
    /// );
    ///
    /// assert_eq!(
    ///     GridRotation::CLOCKWISE.iterate().collect::<Vec<_>>(),
    ///     vec![
    ///         GridRotation::IDENTITY,
    ///         GridRotation::CLOCKWISE,
    ///         GridRotation::CLOCKWISE * GridRotation::CLOCKWISE,
    ///         GridRotation::COUNTERCLOCKWISE,
    ///    ],
    /// );
    /// ```
    pub fn iterate(self) -> impl Iterator<Item = Self> {
        let mut item = Self::IDENTITY;
        std::iter::once(Self::IDENTITY).chain(std::iter::from_fn(move || {
            item = item * self;
            if item == Self::IDENTITY {
                // Cycled back to start; time to stop
                None
            } else {
                Some(item)
            }
        }))
    }
}

impl Default for GridRotation {
    /// Returns the identity (no rotation).
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl One for GridRotation {
    /// Returns the identity (no rotation).
    #[inline]
    fn one() -> Self {
        Self::IDENTITY
    }
}

impl Mul<Self> for GridRotation {
    type Output = Self;

    /// Multiplication is concatenation: `self * rhs` is equivalent to
    /// applying `rhs` and then applying `self`.
    /// ```
    /// use all_is_cubes::math::{Face, Face::*, GridRotation, GridPoint};
    ///
    /// let transform_1 = GridRotation::from_basis([NY, PX, PZ]);
    /// let transform_2 = GridRotation::from_basis([PY, PZ, PX]);
    ///
    /// // Demonstrate the directionality of concatenation.
    /// for &face in Face::ALL_SEVEN {
    ///     assert_eq!(
    ///         (transform_1 * transform_2).transform(face),
    ///         transform_1.transform(transform_2.transform(face)),
    ///     );
    /// }
    /// ```
    #[inline]
    fn mul(self, rhs: Self) -> Self::Output {
        Self::from_basis(rhs.to_basis().map(|v| self.transform(v)))
    }
}

// TODO: consider implementing cgmath::Transform for GridRotation.

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use Face::*;

    #[test]
    fn identity() {
        assert_eq!(GridRotation::IDENTITY, GridRotation::one());
        assert_eq!(GridRotation::IDENTITY, GridRotation::default());
        assert_eq!(
            GridRotation::IDENTITY,
            GridRotation::from_basis([PX, PY, PZ])
        );
    }

    #[test]
    fn ccw_cw() {
        assert_eq!(
            GridRotation::IDENTITY,
            GridRotation::COUNTERCLOCKWISE * GridRotation::CLOCKWISE
        );
    }

    /// Test that `GridRotation::ALL` is complete.
    /// TODO: Also test numbering/ordering properties when that is stable.
    #[test]
    fn enumeration() {
        let mut set = HashSet::new();
        for &rot in &GridRotation::ALL {
            set.insert(rot);
        }
        assert_eq!(set.len(), GridRotation::ALL.len());
        assert_eq!(48, GridRotation::ALL.len());
    }
}