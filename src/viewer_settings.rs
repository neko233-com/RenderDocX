//! Mesh viewer configuration dialogs: axis mapping, projection guess and
//! camera controls. Mirrors the validation rules in upstream's
//! `AxisMappingDialog.cpp`, `ProjectionGuessDialog.cpp` and
//! `CameraControlsDialog.cpp`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct FloatVector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Six selectable directions, matching upstream's combo box order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AxisDirection {
    Right,
    Left,
    Up,
    Down,
    IntoScreen,
    OutOfScreen,
}

impl AxisDirection {
    pub const ALL: [Self; 6] = [
        Self::Right,
        Self::Left,
        Self::Up,
        Self::Down,
        Self::IntoScreen,
        Self::OutOfScreen,
    ];

    pub fn vector(self) -> FloatVector {
        match self {
            Self::Right => FloatVector {
                x: 1.0,
                ..Default::default()
            },
            Self::Left => FloatVector {
                x: -1.0,
                ..Default::default()
            },
            Self::Up => FloatVector {
                y: 1.0,
                ..Default::default()
            },
            Self::Down => FloatVector {
                y: -1.0,
                ..Default::default()
            },
            Self::IntoScreen => FloatVector {
                z: 1.0,
                ..Default::default()
            },
            Self::OutOfScreen => FloatVector {
                z: -1.0,
                ..Default::default()
            },
        }
    }

    pub fn from_vector(vector: &FloatVector) -> Self {
        if vector.x != 0.0 {
            if vector.x < 0.0 {
                Self::Left
            } else {
                Self::Right
            }
        } else if vector.y != 0.0 {
            if vector.y < 0.0 {
                Self::Down
            } else {
                Self::Up
            }
        } else if vector.z < 0.0 {
            Self::OutOfScreen
        } else {
            Self::IntoScreen
        }
    }

    /// Upstream uses `index / 2` to detect degenerate mappings.
    fn axis(self) -> u8 {
        match self {
            Self::Right | Self::Left => 0,
            Self::Up | Self::Down => 1,
            Self::IntoScreen | Self::OutOfScreen => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AxisMapping {
    pub x: AxisDirection,
    pub y: AxisDirection,
    pub z: AxisDirection,
}

impl Default for AxisMapping {
    fn default() -> Self {
        Self {
            x: AxisDirection::Right,
            y: AxisDirection::Up,
            z: AxisDirection::IntoScreen,
        }
    }
}

impl AxisMapping {
    /// Reject mappings where two or more axes share the same direction,
    /// exactly like `AxisMappingDialog::setNewAxisMapping`.
    pub fn validate(self) -> Result<(), AxisError> {
        let (x, y, z) = (self.x.axis(), self.y.axis(), self.z.axis());
        if x != y && y != z && x != z {
            return Ok(());
        }
        let all = x == y && y == z;
        let (first, second, duplicate) = if x == y {
            (AxisErrorAxis::X, AxisErrorAxis::Y, x)
        } else if y == z {
            (AxisErrorAxis::Y, AxisErrorAxis::Z, y)
        } else {
            (AxisErrorAxis::X, AxisErrorAxis::Z, z)
        };
        Err(AxisError {
            all,
            first,
            second,
            direction: duplicate,
        })
    }

    pub fn vectors(self) -> (FloatVector, FloatVector, FloatVector) {
        (self.x.vector(), self.y.vector(), self.z.vector())
    }

    /// Apply the axis mapping to an unprojected world point, producing the
    /// remapped coordinate used by the mesh viewer. Pure component selection.
    pub fn remap(self, point: [f32; 3]) -> [f32; 3] {
        // Choose the source component by which axis family each direction maps to.
        let source = |direction: AxisDirection, p: [f32; 3]| match direction {
            AxisDirection::Right => p[0],
            AxisDirection::Left => -p[0],
            AxisDirection::Up => p[1],
            AxisDirection::Down => -p[1],
            AxisDirection::IntoScreen => p[2],
            AxisDirection::OutOfScreen => -p[2],
        };
        [
            source(self.x, point),
            source(self.y, point),
            source(self.z, point),
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AxisErrorAxis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AxisError {
    pub all: bool,
    pub first: AxisErrorAxis,
    pub second: AxisErrorAxis,
    /// 0 left/right, 1 up/down, 2 into/out of screen.
    pub direction: u8,
}

/// Parsed projection parameters. Empty text fields become 0.0 (estimate from
/// mesh data), matching `ProjectionGuessDialog::validateParameters`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectionGuess {
    pub orthographic: bool,
    pub fov: f32,
    pub aspect: f32,
    pub near_plane: f32,
    pub far_plane: f32,
}

impl Default for ProjectionGuess {
    fn default() -> Self {
        Self {
            orthographic: false,
            fov: 90.0,
            aspect: 0.0,
            near_plane: 0.0,
            far_plane: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    Aspect,
    Near,
    Far,
    NearAfterFar,
}

/// Validate dialog text fields upstream-style. `f32::MAX` represents an
/// infinite far plane (reverse-Z), and orthographic mode ignores FOV/aspect.
pub fn parse_projection(
    orthographic: bool,
    fov: f32,
    aspect_text: &str,
    near_text: &str,
    far_text: &str,
    infinite_far: bool,
) -> Result<ProjectionGuess, ProjectionError> {
    let mut result = ProjectionGuess {
        orthographic,
        ..Default::default()
    };
    if orthographic {
        result.aspect = 0.0;
        result.fov = 90.0;
    } else {
        result.aspect = parse_optional_float(aspect_text).ok_or(ProjectionError::Aspect)?;
        result.fov = fov;
    }
    result.near_plane = parse_optional_float(near_text).ok_or(ProjectionError::Near)?;
    result.far_plane = if infinite_far {
        f32::MAX
    } else {
        parse_optional_float(far_text).ok_or(ProjectionError::Far)?
    };
    if result.near_plane > 0.0 && result.far_plane > 0.0 && result.near_plane >= result.far_plane {
        return Err(ProjectionError::NearAfterFar);
    }
    Ok(result)
}

fn parse_optional_float(text: &str) -> Option<f32> {
    if text.trim().is_empty() {
        Some(0.0)
    } else {
        text.trim().parse().ok()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SpeedModifier {
    Shift,
    Alt,
    Control,
    None,
}

impl SpeedModifier {
    pub const ALL: [Self; 4] = [Self::Shift, Self::Alt, Self::Control, Self::None];
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct CameraControls {
    pub near_plane: f32,
    pub far_plane: f32,
    pub speed_modifier: SpeedModifier,
}

impl Default for CameraControls {
    fn default() -> Self {
        Self {
            near_plane: 0.01,
            far_plane: 10000.0,
            speed_modifier: SpeedModifier::Shift,
        }
    }
}

impl CameraControls {
    pub fn validate(self) -> Result<(), CameraError> {
        if !self.near_plane.is_finite()
            || !self.far_plane.is_finite()
            || self.near_plane <= 0.0
            || self.far_plane <= 0.0
        {
            return Err(CameraError::InvalidPlanes);
        }
        if self.near_plane >= self.far_plane {
            return Err(CameraError::NearAfterFar);
        }
        Ok(())
    }
}

/// Build the unprojection projection matrix from the user's ProjectionGuess
/// parameters (aspect 0 means estimate 1.0, near/far 0 means estimate defaults).
pub fn projection_matrix(params: &ProjectionGuess) -> Option<crate::projection::Mat4> {
    let aspect = if params.aspect > 0.0 {
        params.aspect
    } else {
        1.0
    };
    let near = if params.near_plane > 0.0 {
        params.near_plane
    } else {
        0.1
    };
    let far = if params.far_plane > 0.0 && params.far_plane.is_finite() {
        params.far_plane
    } else {
        100.0
    };
    if params.orthographic {
        crate::projection::orthographic(aspect * 2.0, 2.0, near, far)
    } else {
        crate::projection::perspective(params.fov, aspect, near, far)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraError {
    InvalidPlanes,
    NearAfterFar,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_mapping_round_trips_and_rejects_degenerate_inputs() {
        for direction in AxisDirection::ALL {
            assert_eq!(AxisDirection::from_vector(&direction.vector()), direction);
        }
        let mapping = AxisMapping::default();
        assert!(mapping.validate().is_ok());
        assert_eq!(
            mapping.vectors(),
            (
                FloatVector {
                    x: 1.0,
                    ..Default::default()
                },
                FloatVector {
                    y: 1.0,
                    ..Default::default()
                },
                FloatVector {
                    z: 1.0,
                    ..Default::default()
                },
            )
        );
        let error = AxisMapping {
            x: AxisDirection::Right,
            y: AxisDirection::Left,
            z: AxisDirection::Up,
        }
        .validate()
        .unwrap_err();
        assert!(error.all == false);
        assert_eq!(error.direction, 0);
        assert_eq!(error.first, AxisErrorAxis::X);
        assert_eq!(error.second, AxisErrorAxis::Y);
        assert!(AxisMapping {
            x: AxisDirection::Right,
            y: AxisDirection::Up,
            z: AxisDirection::Down
        }
        .validate()
        .is_err());
    }

    #[test]
    fn projection_parsing_follows_upstream_validation() {
        assert_eq!(
            parse_projection(false, 75.0, "1.777", "0.1", "100", false).unwrap(),
            ProjectionGuess {
                orthographic: false,
                fov: 75.0,
                aspect: 1.777,
                near_plane: 0.1,
                far_plane: 100.0
            }
        );
        assert_eq!(
            parse_projection(false, 90.0, "", "", "", true)
                .unwrap()
                .far_plane,
            f32::MAX
        );
        let ortho = parse_projection(true, 10.0, "bad", "0.1", "100", false).unwrap();
        assert!(ortho.orthographic && ortho.aspect == 0.0 && ortho.fov == 90.0);
        assert_eq!(
            parse_projection(true, 10.0, "", "bad", "", false),
            Err(ProjectionError::Near)
        );
        assert_eq!(
            parse_projection(false, 90.0, "bad", "", "", false),
            Err(ProjectionError::Aspect)
        );
        assert_eq!(
            parse_projection(false, 90.0, "", "bad", "", false),
            Err(ProjectionError::Near)
        );
        assert_eq!(
            parse_projection(false, 90.0, "", "", "bad", false),
            Err(ProjectionError::Far)
        );
        assert_eq!(
            parse_projection(false, 90.0, "", "2", "1", false),
            Err(ProjectionError::NearAfterFar)
        );
    }

    #[test]
    fn axis_mapping_remaps_unprojected_point() {
        let identity = AxisMapping::default(); // x=Right, y=Up, z=IntoScreen
        assert_eq!(identity.remap([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
        let flipped = AxisMapping {
            x: AxisDirection::Left,
            y: AxisDirection::Down,
            z: AxisDirection::OutOfScreen,
        };
        assert_eq!(flipped.remap([1.0, 2.0, 3.0]), [-1.0, -2.0, -3.0]);
    }

    #[test]
    fn projection_matrix_from_guess_is_invertible() {
        let params = ProjectionGuess {
            orthographic: false,
            fov: 75.0,
            aspect: 16.0 / 9.0,
            near_plane: 0.0,
            far_plane: 0.0,
        };
        let matrix = projection_matrix(&params).unwrap();
        assert!(matrix.inverse().is_some());
        let ortho = ProjectionGuess {
            orthographic: true,
            ..params
        };
        assert!(projection_matrix(&ortho).unwrap().inverse().is_some());
    }

    #[test]
    fn camera_planes_are_validated() {
        assert!(CameraControls::default().validate().is_ok());
        assert_eq!(
            CameraControls {
                near_plane: 5.0,
                far_plane: 1.0,
                ..Default::default()
            }
            .validate(),
            Err(CameraError::NearAfterFar)
        );
        assert_eq!(
            CameraControls {
                near_plane: 0.0,
                far_plane: 1.0,
                ..Default::default()
            }
            .validate(),
            Err(CameraError::InvalidPlanes)
        );
    }
}
