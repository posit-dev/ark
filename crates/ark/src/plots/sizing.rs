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

/// Default DPI for converting inches to pixels.
/// Matches R's default: 96 on macOS, 72 on Linux/Windows.
/// See `default_resolution_in_pixels_per_inch()` in graphics.R.
pub(crate) const DEFAULT_DPI: f64 = if cfg!(target_os = "macos") {
    96.0
} else {
    72.0
};

/// Default figure size in inches, matching Quarto's base/HTML format
/// defaults (`fig-width: 7`, `fig-height: 5`). Other formats differ (e.g.
/// pdf is 5.5 x 3.5), but Positron doesn't know the target format.
const DEFAULT_FIG_WIDTH: f64 = 7.0;
const DEFAULT_FIG_HEIGHT: f64 = 5.0;

/// Plot sizing metadata from an execute request.
///
/// Holds the *unresolved* sizing request: optional figure dimensions in
/// inches, where "unset" survives until render time so the `ark.plot.*` R
/// options can be layered per dimension. Contrast with [PlotRenderSettings],
/// which describes a fully resolved render in concrete pixels.
#[derive(Clone, Copy, Default)]
pub(crate) struct PlotSizing {
    /// Figure width in inches requested by the execute request (Quarto's
    /// `fig-width`), validated positive.
    pub(crate) fig_width: Option<f64>,
    /// Figure height in inches requested by the execute request (Quarto's
    /// `fig-height`), validated positive.
    pub(crate) fig_height: Option<f64>,
    /// Device pixel ratio of the frontend's display (`output_pixel_ratio`).
    pub(crate) pixel_ratio: Option<f64>,
}

impl PlotSizing {
    /// Extract sizing metadata from an execute request, discarding
    /// non-positive dimensions.
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

    /// Render settings at the requested figure size.
    ///
    /// Returns `None` when no figure size was requested, so the caller can
    /// fall back to its own default (e.g. the frontend-driven prerender
    /// settings).
    pub(crate) fn requested_render_settings(&self) -> Option<PlotRenderSettings> {
        let (width, height) = self.fig_size()?;

        Some(render_settings_from_inches(
            width,
            height,
            self.pixel_ratio.unwrap_or(1.0),
        ))
    }

    /// Resolve final render settings for the Jupyter protocol.
    ///
    /// Each dimension resolves independently: the `ark.plot.*` R options
    /// override the execute request's figure size, which overrides the
    /// default figure size. Unsized plots deliberately render at the default
    /// figure size rather than scaling with the output area
    /// (posit-dev/positron#15260).
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

    /// The requested figure size as an `IntrinsicSize` for the plot comm.
    ///
    /// Returns `None` when no figure size was requested.
    pub(crate) fn intrinsic_size(&self) -> Option<IntrinsicSize> {
        let (width, height) = self.fig_size()?;

        Some(IntrinsicSize {
            width,
            height,
            unit: PlotUnit::Inches,
            source: String::from("Quarto"),
        })
    }

    /// The figure size requested via `fig-width`/`fig-height`, in inches.
    ///
    /// Either option may be set alone, matching `quarto render`; the missing
    /// dimension falls back to the Quarto default. Returns `None` when
    /// neither dimension is set.
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
    /// Convert an intrinsic size to a logical-pixel-based `PlotSize`.
    ///
    /// Returns dimensions in CSS/logical pixels. The R rendering layer handles
    /// physical pixel scaling via the separate `pixel_ratio` parameter.
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

/// Build PNG render settings from a figure size in inches.
///
/// The size is converted to CSS/logical pixels (inches * DPI). The R
/// rendering layer handles physical pixel scaling via the separate
/// `pixel_ratio` parameter.
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

/// Read a positive `f64` from an R option. Returns `None` if the option is
/// unset, not numeric, or not positive.
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
