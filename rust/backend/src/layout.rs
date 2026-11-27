//! Pure geometry and ratio handling for Rex's tiled session layout.

/// A rectangle in normalized workspace coordinates: `[x, y, width, height]`.
pub type Rect = [f64; 4];

/// The direction of a split, using Rex's server vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitDirection {
    /// Divide width, with `before` on the left and `after` on the right.
    Horizontal,
    /// Divide height, with `before` above and `after` below.
    Vertical,
}

/// The two rectangles produced by a split.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitRects {
    pub before: Rect,
    pub after: Rect,
}

/// Normalize a ratio supplied when creating a split.
///
/// Go treats an omitted/zero ratio as one half, and otherwise requires a
/// strictly interior ratio. In particular, creation does not clamp values.
#[must_use]
pub fn normalize_creation_ratio(ratio: f64) -> Option<f64> {
    if ratio == 0.0 {
        Some(0.5)
    } else {
        normalize_resize_ratio(ratio)
    }
}

/// Normalize a ratio supplied when resizing an existing split.
///
/// Unlike split creation, resize does not give zero a default value. Go accepts
/// only ratios strictly between zero and one and performs no clamping.
#[must_use]
pub fn normalize_resize_ratio(ratio: f64) -> Option<f64> {
    (0.0 < ratio && ratio < 1.0).then_some(ratio)
}

/// Split `rect` while preserving the exact coordinate convention used by Go:
/// `before` gets `ratio` of the relevant extent and `after` gets the remainder.
///
/// A non-interior, NaN, or infinite ratio is rejected rather than clamped.
#[must_use]
pub fn split_rect(rect: Rect, direction: SplitDirection, ratio: f64) -> Option<SplitRects> {
    let ratio = normalize_resize_ratio(ratio)?;
    let [x, y, width, height] = rect;

    match direction {
        SplitDirection::Horizontal => {
            let before_width = width * ratio;
            Some(SplitRects {
                before: [x, y, before_width, height],
                after: [x + before_width, y, width - before_width, height],
            })
        }
        SplitDirection::Vertical => {
            let before_height = height * ratio;
            Some(SplitRects {
                before: [x, y, width, before_height],
                after: [x, y + before_height, width, height - before_height],
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Rect, SplitDirection, normalize_creation_ratio, normalize_resize_ratio, split_rect,
    };

    fn assert_rect(actual: Rect, expected: Rect) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
        }
    }

    #[test]
    fn creation_defaults_zero_but_resize_rejects_it() {
        assert_eq!(normalize_creation_ratio(0.0), Some(0.5));
        assert_eq!(normalize_creation_ratio(-0.0), Some(0.5));
        assert_eq!(normalize_resize_ratio(0.0), None);
        assert_eq!(normalize_resize_ratio(-0.0), None);
        assert_eq!(normalize_creation_ratio(0.25), Some(0.25));
        assert_eq!(normalize_resize_ratio(0.25), Some(0.25));
    }

    #[test]
    fn server_ratios_are_strict_and_are_not_clamped() {
        for ratio in [
            f64::NEG_INFINITY,
            -1.0,
            -0.01,
            1.0,
            2.0,
            f64::INFINITY,
            f64::NAN,
        ] {
            assert_eq!(normalize_creation_ratio(ratio), None);
            assert_eq!(normalize_resize_ratio(ratio), None);
        }
        assert_eq!(
            normalize_creation_ratio(f64::from_bits(1)),
            Some(f64::from_bits(1))
        );
        assert_eq!(
            normalize_resize_ratio(1.0 - f64::EPSILON),
            Some(1.0 - f64::EPSILON)
        );
    }

    #[test]
    fn horizontal_split_changes_only_x_and_width() {
        let split = split_rect([0.2, 0.1, 0.8, 0.9], SplitDirection::Horizontal, 0.25).unwrap();
        assert_rect(split.before, [0.2, 0.1, 0.2, 0.9]);
        assert_rect(split.after, [0.4, 0.1, 0.6, 0.9]);
    }

    #[test]
    fn vertical_split_changes_only_y_and_height() {
        let split = split_rect([0.2, 0.1, 0.8, 0.9], SplitDirection::Vertical, 0.25).unwrap();
        assert_rect(split.before, [0.2, 0.1, 0.8, 0.225]);
        assert_rect(split.after, [0.2, 0.325, 0.8, 0.675]);
    }

    #[test]
    fn nested_regions_inherit_their_parent_region() {
        let columns = split_rect([0.2, 0.1, 0.8, 0.9], SplitDirection::Horizontal, 0.75).unwrap();
        let rows = split_rect(columns.after, SplitDirection::Vertical, 0.4).unwrap();

        assert_rect(columns.before, [0.2, 0.1, 0.6, 0.9]);
        assert_rect(rows.before, [0.8, 0.1, 0.2, 0.36]);
        assert_rect(rows.after, [0.8, 0.46, 0.2, 0.54]);
    }

    #[test]
    fn preserves_ratios_near_the_edges() {
        let low = split_rect([0.0, 0.0, 1.0, 1.0], SplitDirection::Vertical, 0.05).unwrap();
        let high = split_rect([0.0, 0.0, 1.0, 1.0], SplitDirection::Vertical, 0.95).unwrap();
        assert_rect(low.after, [0.0, 0.05, 1.0, 0.95]);
        assert_rect(high.after, [0.0, 0.95, 1.0, 0.05]);
    }

    #[test]
    fn split_geometry_rejects_server_invalid_boundaries() {
        for ratio in [0.0, 1.0, -0.1, 1.1, f64::NAN] {
            assert!(split_rect([0.0, 0.0, 1.0, 1.0], SplitDirection::Horizontal, ratio).is_none());
        }
    }
}
