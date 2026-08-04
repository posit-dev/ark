//
// sizing.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//

use amalthea::comm::plot_comm::IntrinsicSize;
use amalthea::comm::plot_comm::PlotRenderFormat;
use amalthea::comm::plot_comm::PlotRenderSettings;
use amalthea::comm::plot_comm::PlotSize;
use amalthea::comm::plot_comm::PlotUnit;
use amalthea::wire::execute_request::ExecuteRequestPositron;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;

use crate::r_task;

/// Default conversion from inches to logical pixels: 96 DPI on macOS and
/// 72 DPI elsewhere, matching `default_resolution_in_pixels_per_inch()` in
/// `graphics.R`.
pub(crate) const DEFAULT_DPI: f64 = if cfg!(target_os = "macos") {
    96.0
} else {
    72.0
};

/// Default figure dimensions for Quarto base and HTML formats, in inches.
///
/// Positron cannot account for format-specific defaults such as PDF's
/// 5.5 × 3.5 inches, so Ark consistently uses 7 × 5 inches.
const DEFAULT_FIG_WIDTH: f64 = 7.0;
const DEFAULT_FIG_HEIGHT: f64 = 5.0;

/// Unresolved plot dimensions and pixel ratio from an execute request.
///
/// Keeping each dimension optional allows it to be resolved independently at
/// render time. [`PlotRenderSettings`] instead represents fully resolved,
/// pixel-based settings.
#[derive(Clone, Copy, Default)]
pub(crate) struct PlotSizing {
    /// Requested `fig-width` in inches, if positive.
    pub(crate) fig_width: Option<f64>,
    /// Requested `fig-height` in inches, if positive.
    pub(crate) fig_height: Option<f64>,
    /// Requested frontend device pixel ratio.
    pub(crate) pixel_ratio: Option<f64>,
}

impl PlotSizing {
    /// Extracts plot sizing metadata, ignoring non-positive figure dimensions.
    pub(crate) fn from_request(positron: Option<&ExecuteRequestPositron>) -> Self {
        let fig_width = positron
            .and_then(|req| req.fig_width)
            .filter(|width| *width > 0.0);
        let fig_height = positron
            .and_then(|req| req.fig_height)
            .filter(|height| *height > 0.0);
        let pixel_ratio = positron.and_then(|req| req.output_pixel_ratio);

        Self {
            fig_width,
            fig_height,
            pixel_ratio,
        }
    }

    /// Resolves explicitly requested figure dimensions for prerendering.
    ///
    /// Returns `None` if neither dimension was requested, allowing the caller to
    /// use its frontend-driven prerender settings.
    pub(crate) fn requested_render_settings(&self) -> Option<PlotRenderSettings> {
        let (width, height) = self.fig_size()?;

        Some(render_settings_from_inches(
            width,
            height,
            self.pixel_ratio.unwrap_or(1.0),
        ))
    }

    /// Resolves the final PNG render settings.
    ///
    /// Each figure dimension independently uses the first positive value from its
    /// `ark.plot.*` R option, the execute request, or the default figure size. The
    /// pixel ratio follows the analogous option-request-default precedence.
    /// `output_width_px` does not affect the rendered dimensions.
    pub(crate) fn resolved_render_settings(&self) -> PlotRenderSettings {
        let width = r_option_positive_f64("ark.plot.width")
            .or(self.fig_width)
            .unwrap_or(DEFAULT_FIG_WIDTH);
        let height = r_option_positive_f64("ark.plot.height")
            .or(self.fig_height)
            .unwrap_or(DEFAULT_FIG_HEIGHT);
        let pixel_ratio = r_option_positive_f64("ark.plot.pixel_ratio")
            .or(self.pixel_ratio)
            .unwrap_or(1.0);

        render_settings_from_inches(width, height, pixel_ratio)
    }

    /// Returns the requested dimensions as plot-comm intrinsic-size metadata.
    ///
    /// Returns `None` if neither dimension was requested.
    pub(crate) fn intrinsic_size(&self) -> Option<IntrinsicSize> {
        let (width, height) = self.fig_size()?;

        Some(IntrinsicSize {
            width,
            height,
            unit: PlotUnit::Inches,
            source: String::from("Quarto"),
        })
    }

    /// Returns the requested figure dimensions in inches.
    ///
    /// A missing dimension uses its corresponding default. Returns `None` if
    /// neither dimension was requested.
    fn fig_size(&self) -> Option<(f64, f64)> {
        if self.fig_width.is_none() && self.fig_height.is_none() {
            return None;
        }

        Some((
            self.fig_width.unwrap_or(DEFAULT_FIG_WIDTH),
            self.fig_height.unwrap_or(DEFAULT_FIG_HEIGHT),
        ))
    }
}

pub(crate) trait IntrinsicSizeExt {
    /// Converts intrinsic dimensions to logical pixels.
    ///
    /// Physical pixel scaling is applied separately through `pixel_ratio`.
    fn to_plot_size(&self) -> PlotSize;
}

impl IntrinsicSizeExt for IntrinsicSize {
    fn to_plot_size(&self) -> PlotSize {
        match self.unit {
            PlotUnit::Inches => PlotSize {
                width: (self.width * DEFAULT_DPI).round() as i64,
                height: (self.height * DEFAULT_DPI).round() as i64,
            },
            PlotUnit::Pixels => PlotSize {
                width: self.width.round() as i64,
                height: self.height.round() as i64,
            },
        }
    }
}

/// Converts figure dimensions in inches into PNG settings in logical pixels.
///
/// Physical pixel scaling is applied separately through `pixel_ratio`.
fn render_settings_from_inches(width: f64, height: f64, pixel_ratio: f64) -> PlotRenderSettings {
    PlotRenderSettings {
        size: PlotSize {
            width: (width * DEFAULT_DPI).round() as i64,
            height: (height * DEFAULT_DPI).round() as i64,
        },
        pixel_ratio,
        format: PlotRenderFormat::Png,
    }
}

/// Reads a positive numeric R option.
///
/// Returns `None` when the option is absent, non-numeric, or non-positive.
fn r_option_positive_f64(name: &str) -> Option<f64> {
    let value = r_task(|| {
        RFunction::from("getOption")
            .param("x", name)
            .call()?
            .to::<f64>()
    });
    match value {
        Ok(v) if v > 0.0 => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_request_filters_non_positive_dimensions() {
        let req = ExecuteRequestPositron {
            fig_width: Some(-1.0),
            fig_height: Some(0.0),
            output_pixel_ratio: Some(2.0),
            ..Default::default()
        };

        let sizing = PlotSizing::from_request(Some(&req));
        assert!(sizing.fig_width.is_none());
        assert!(sizing.fig_height.is_none());
        assert_eq!(sizing.pixel_ratio, Some(2.0));
    }

    #[test]
    fn test_fig_size_lone_dimension_uses_default() {
        let sizing = PlotSizing {
            fig_width: Some(4.0),
            ..Default::default()
        };

        assert_eq!(sizing.fig_size(), Some((4.0, DEFAULT_FIG_HEIGHT)));
    }

    #[test]
    fn test_intrinsic_size_none_without_fig_size() {
        let sizing = PlotSizing::default();
        assert!(sizing.intrinsic_size().is_none());
    }
}
