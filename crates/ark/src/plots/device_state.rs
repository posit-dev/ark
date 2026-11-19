//
// device_state.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//

//! Hold and notification state of the Ark graphics device.
//!
//! The state has two parts: the state of the current page ([PageState]) and
//! the `dev.hold()` state of the device ([Hold]). Each graphics hook is one
//! transition on [DeviceState]. The transitions never call into R. Instead,
//! they tell [DeviceContext](crate::plots::graphics_device::DeviceContext)
//! which page to record or which notification to send, and it does the work.

use std::mem;

use crate::plots::graphics_device::PlotId;

#[derive(Debug)]
pub(crate) struct DeviceState {
    page: Page,
    hold: Hold,
}

#[derive(Debug)]
struct Page {
    /// Used for looking up a recorded plot so we can replay it with different
    /// graphics device specifications (i.e. for Positron's Plots pane).
    id: PlotId,

    /// Whether the frontend has been sent this page. The next notification
    /// for a page that wasn't shown opens a new plot, and later ones update
    /// it.
    shown: bool,

    state: PageState,
}

/// What the frontend and the latest recording have of the current page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageState {
    /// The frontend has everything drawn on the page, or nothing is drawn yet.
    Synced,

    /// The page has drawing that the frontend doesn't have, and that the
    /// latest recording may not include.
    Drawn,

    /// The page was recorded while the device was held, and its notification
    /// waits until the hold is released.
    ///
    /// `redrawn` is `true` if the page has drawing that the recording doesn't
    /// include, for example because drawing continued after the recording or
    /// because the recording failed.
    Deferred { redrawn: bool },
}

/// The `dev.hold()` state of the device.
#[derive(Debug)]
enum Hold {
    /// The `dev.hold()` level is 0 or less, so notifications are sent at once.
    Released,

    Held {
        /// Whether the current page had unsent changes when the hold started.
        ///
        /// On a screen device, drawing done before a hold stays visible, so R
        /// can stop for input while held and the user still sees it. The
        /// `devAskNewPage()` prompt is the common case: `plot.default()` holds
        /// the device before `plot.new()` asks "Hit <Return> to see next
        /// plot" (posit-dev/positron#7680).
        has_pre_hold_changes: bool,

        /// Earlier pages that need a final update when the hold is released.
        ///
        /// Code that holds the device around `plot.new()`, for example
        /// `plot.default()` or `bxp()`, means the `before.plot.new` hook can
        /// only record the previous page and defer its notification. When the
        /// new page starts, the previous page moves here so its final state
        /// still reaches the frontend after the release, instead of being lost
        /// (posit-dev/positron#17240).
        pages: Vec<PlotId>,
    },
}

/// A notification to send to the frontend for a recorded page.
#[derive(Debug, PartialEq)]
pub(crate) enum Notification {
    /// Open a new plot for a page the frontend hasn't seen.
    New(PlotId),

    /// Update a plot the frontend has already seen.
    Update(PlotId),
}

/// A page to send before R waits for input while the device is held. Pass it
/// back to [DeviceState::notify_pre_hold_changes] once the page has a
/// recording.
#[derive(Debug, PartialEq)]
pub(crate) struct PreHoldChanges {
    pub id: PlotId,
    recording: PreHoldRecording,
}

#[derive(Debug, PartialEq)]
enum PreHoldRecording {
    /// The page has drawing that its latest recording may not include.
    Needed,

    /// The page was deferred, so it has a recording already. `redrawn` is the
    /// page's [PageState::Deferred] flag.
    Done { redrawn: bool },
}

impl PreHoldChanges {
    /// Whether to record the page before sending it.
    pub(crate) fn needs_recording(&self) -> bool {
        self.recording == PreHoldRecording::Needed
    }
}

/// The notification for [PreHoldChanges], and what to do after it is sent.
#[derive(Debug, PartialEq)]
pub(crate) struct PreHoldNotification {
    pub notification: Notification,

    /// Whether the sent recording misses drawing done while held. If so, the
    /// release records and sends the page again.
    pub redrawn: bool,
}

impl DeviceState {
    pub(crate) fn new() -> Self {
        Self {
            page: Page::new(PageState::Synced),
            hold: Hold::Released,
        }
    }

    /// The ID of the current page.
    pub(crate) fn id(&self) -> &PlotId {
        &self.page.id
    }

    pub(crate) fn is_held(&self) -> bool {
        matches!(self.hold, Hold::Held { .. })
    }

    /// The ID of the current page if it has changes that the frontend
    /// doesn't have. This page needs to be recorded and then passed to
    /// [DeviceState::on_recorded].
    pub(crate) fn changed_page(&self) -> Option<&PlotId> {
        match self.page.state {
            PageState::Synced => None,
            PageState::Drawn | PageState::Deferred { .. } => Some(&self.page.id),
        }
    }

    /// The device's mode changed to drawing.
    pub(crate) fn on_draw(&mut self) {
        self.page.state = match self.page.state {
            PageState::Synced | PageState::Drawn => PageState::Drawn,
            PageState::Deferred { .. } => PageState::Deferred { redrawn: true },
        };
    }

    /// The device's `dev.hold()` level changed to `level`.
    ///
    /// Returns the held pages if this releases the hold. The caller processes
    /// the current page and then sends an update for each held page.
    pub(crate) fn on_hold_level(&mut self, level: i32) -> Option<Vec<PlotId>> {
        // Be extra safe and check `level <= 0` rather than just `level == 0`
        // in case our shadowed device returns a negative `level`
        let is_released = level <= 0;

        match (&mut self.hold, is_released) {
            (Hold::Released, true) | (Hold::Held { .. }, false) => None,
            (Hold::Released, false) => {
                self.hold = Hold::Held {
                    has_pre_hold_changes: self.page.state != PageState::Synced,
                    pages: Vec::new(),
                };
                None
            },
            (Hold::Held { pages, .. }, true) => {
                let held_pages = mem::take(pages);
                self.hold = Hold::Released;
                Some(held_pages)
            },
        }
    }

    /// A new page started.
    ///
    /// Only pages the frontend has already seen are owed an update. A page
    /// that was never shown stays hidden, so `dev.hold()` still suppresses
    /// intermediate pages.
    pub(crate) fn on_new_page(&mut self) {
        // Unsent changes carry over to the new page, so it is sent even if
        // nothing is drawn on it. `before.plot.new` and `before.grid.newpage`
        // process the page first, so normally this only happens to a page
        // deferred while held, and the new page is sent at release.
        let state = match self.page.state {
            PageState::Synced => PageState::Synced,
            PageState::Drawn | PageState::Deferred { .. } => PageState::Drawn,
        };
        let previous = mem::replace(&mut self.page, Page::new(state));

        let Hold::Held {
            has_pre_hold_changes,
            pages,
        } = &mut self.hold
        else {
            return;
        };
        *has_pre_hold_changes = false;

        if matches!(previous.state, PageState::Deferred { .. }) && previous.shown {
            pages.push(previous.id);
        }
    }

    /// R is about to wait for a click on the current page, for `locator()`.
    ///
    /// A page that `plot.new()` or `grid::grid.newpage()` started is blank
    /// until something is drawn on it, so it isn't a plot change yet. If the
    /// frontend hasn't been sent it, mark it changed so it is sent anyway and
    /// there is a plot to click on. A held page stays hidden.
    pub(crate) fn on_locate(&mut self) {
        if !self.page.shown && !self.is_held() && self.page.state == PageState::Synced {
            self.page.state = PageState::Drawn;
        }
    }

    /// The frontend closed the plot `id`.
    pub(crate) fn on_plot_closed(&mut self, id: &PlotId) {
        if let Hold::Held { pages, .. } = &mut self.hold {
            pages.retain(|held| held != id);
        }

        // If the current page is closed, move to a new page that the frontend
        // hasn't seen (posit-dev/positron#6702)
        if self.page.id == *id {
            self.page.id = PlotId::new();
            self.page.shown = false;
        }
    }

    /// The page `id` from [DeviceState::changed_page] was recorded. `recorded`
    /// is `false` if the recording failed.
    ///
    /// Returns the notification to send, or `None` if the notification is
    /// deferred because the device is held.
    pub(crate) fn on_recorded(&mut self, id: &PlotId, recorded: bool) -> Option<Notification> {
        if self.is_held() {
            // A failed recording leaves an earlier recording of a deferred
            // page in place, so that page keeps its `redrawn` flag
            let redrawn = match self.page.state {
                _ if recorded => false,
                PageState::Deferred { redrawn } => redrawn,
                PageState::Synced | PageState::Drawn => true,
            };
            self.page.state = PageState::Deferred { redrawn };
            return None;
        }
        Some(self.notify(id))
    }

    /// Take the changes to send before R waits for input while the device is
    /// held.
    ///
    /// A page is sent only if it had changes drawn before the hold, so
    /// `dev.hold()` keeps hiding pages that were drawn entirely while held.
    pub(crate) fn take_pre_hold_changes(&mut self) -> Option<PreHoldChanges> {
        let Hold::Held {
            has_pre_hold_changes,
            ..
        } = &mut self.hold
        else {
            return None;
        };

        if !mem::take(has_pre_hold_changes) {
            return None;
        }

        let recording = match self.page.state {
            PageState::Synced => return None,
            PageState::Drawn => PreHoldRecording::Needed,
            PageState::Deferred { redrawn } => PreHoldRecording::Done { redrawn },
        };

        Some(PreHoldChanges {
            id: self.page.id.clone(),
            recording,
        })
    }

    /// The page from [DeviceState::take_pre_hold_changes] has a recording.
    /// `recorded` is `false` if the page needed a recording and it failed.
    ///
    /// After the notification is sent, pass `redrawn` to
    /// [DeviceState::on_pre_hold_changes_sent].
    pub(crate) fn notify_pre_hold_changes(
        &mut self,
        changes: PreHoldChanges,
        recorded: bool,
    ) -> PreHoldNotification {
        let redrawn = match changes.recording {
            PreHoldRecording::Needed => !recorded,
            PreHoldRecording::Done { redrawn } => redrawn,
        };

        PreHoldNotification {
            notification: self.notify(&changes.id),
            redrawn,
        }
    }

    /// The notification from [DeviceState::notify_pre_hold_changes] was sent.
    ///
    /// If the sent recording misses drawing done while held, for example a
    /// second panel with `par(mfrow =)`, the page keeps its changes so the
    /// release records and sends it again. The changes stay with the current
    /// page even if the frontend closed the sent plot during the render.
    ///
    /// Rendering for the notification can also draw back into our device, for
    /// example when a replayed `recordGraphics()` expression draws on the
    /// current device. The nested `process_changes()` then records and
    /// defers the page. That newer state wins, so the page still gets its
    /// update at release.
    pub(crate) fn on_pre_hold_changes_sent(&mut self, redrawn: bool) {
        if redrawn && self.page.state == PageState::Synced {
            self.page.state = PageState::Drawn;
        }
    }

    /// Mark the current page as sent and return the notification for `id`,
    /// the page the caller recorded.
    fn notify(&mut self, id: &PlotId) -> Notification {
        self.page.state = PageState::Synced;

        let id = id.clone();
        match mem::replace(&mut self.page.shown, true) {
            true => Notification::Update(id),
            false => Notification::New(id),
        }
    }
}

impl Page {
    fn new(state: PageState) -> Self {
        Self {
            id: PlotId::new(),
            shown: false,
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::slice;

    use super::*;

    /// Draw on the current page and record it, as `process_changes()` does.
    fn draw_and_process(state: &mut DeviceState) -> Option<Notification> {
        state.on_draw();
        record(state, true)
    }

    /// Record the changed page, as `process_changes()` does.
    fn record(state: &mut DeviceState, recorded: bool) -> Option<Notification> {
        let Some(id) = state.changed_page().cloned() else {
            panic!("The current page has no changes to record");
        };
        state.on_recorded(&id, recorded)
    }

    fn held_pages(state: &DeviceState) -> &[PlotId] {
        match &state.hold {
            Hold::Released => &[],
            Hold::Held { pages, .. } => pages,
        }
    }

    #[test]
    fn test_new_page_has_no_changes() {
        let state = DeviceState::new();
        assert_eq!(state.changed_page(), None);
        assert!(!state.is_held());
    }

    #[test]
    fn test_released_page_is_new_then_updated() {
        let mut state = DeviceState::new();
        let id = state.id().clone();

        assert_eq!(
            draw_and_process(&mut state),
            Some(Notification::New(id.clone()))
        );
        assert_eq!(state.changed_page(), None);

        assert_eq!(draw_and_process(&mut state), Some(Notification::Update(id)));
    }

    #[test]
    fn test_held_page_is_deferred_until_release() {
        let mut state = DeviceState::new();
        let id = state.id().clone();

        assert_eq!(state.on_hold_level(1), None);
        assert!(state.is_held());
        assert_eq!(draw_and_process(&mut state), None);
        assert_eq!(state.page.state, PageState::Deferred { redrawn: false });

        // Nested holds don't change anything
        assert_eq!(state.on_hold_level(2), None);
        assert_eq!(state.on_hold_level(1), None);
        assert!(state.is_held());

        assert_eq!(state.on_hold_level(0), Some(Vec::new()));
        assert_eq!(state.changed_page(), Some(&id));
        assert_eq!(record(&mut state, true), Some(Notification::New(id)));
    }

    #[test]
    fn test_negative_hold_level_releases() {
        let mut state = DeviceState::new();
        state.on_hold_level(1);
        assert_eq!(state.on_hold_level(-1), Some(Vec::new()));
        assert!(!state.is_held());
    }

    #[test]
    fn test_drawing_after_deferral_marks_page_redrawn() {
        let mut state = DeviceState::new();
        state.on_hold_level(1);
        draw_and_process(&mut state);

        state.on_draw();
        assert_eq!(state.page.state, PageState::Deferred { redrawn: true });
    }

    #[test]
    fn test_failed_recording_while_held_marks_page_redrawn() {
        let mut state = DeviceState::new();
        state.on_hold_level(1);
        state.on_draw();

        assert_eq!(record(&mut state, false), None);
        assert_eq!(state.page.state, PageState::Deferred { redrawn: true });
    }

    #[test]
    fn test_failed_rerecording_while_held_keeps_earlier_recording() {
        let mut state = DeviceState::new();
        state.on_hold_level(1);
        draw_and_process(&mut state);

        assert_eq!(record(&mut state, false), None);
        assert_eq!(state.page.state, PageState::Deferred { redrawn: false });
    }

    #[test]
    fn test_held_new_page_keeps_shown_deferred_page() {
        let mut state = DeviceState::new();
        let first = state.id().clone();
        draw_and_process(&mut state);

        state.on_hold_level(1);
        draw_and_process(&mut state);
        state.on_new_page();
        let second = state.id().clone();

        assert_ne!(first, second);
        assert_eq!(held_pages(&state), slice::from_ref(&first));

        // The new page starts with changes, so it is sent at release
        assert_eq!(state.page.state, PageState::Drawn);
        assert_eq!(state.on_hold_level(0), Some(vec![first]));
        assert_eq!(record(&mut state, true), Some(Notification::New(second)));
    }

    #[test]
    fn test_held_new_page_drops_unshown_deferred_page() {
        let mut state = DeviceState::new();
        state.on_hold_level(1);
        draw_and_process(&mut state);
        state.on_new_page();

        assert_eq!(held_pages(&state), []);
    }

    #[test]
    fn test_released_new_page_has_no_changes() {
        let mut state = DeviceState::new();
        draw_and_process(&mut state);
        state.on_new_page();

        assert_eq!(state.changed_page(), None);
        assert!(!state.page.shown);
    }

    #[test]
    fn test_locate_sends_blank_unshown_page() {
        let mut state = DeviceState::new();
        let id = state.id().clone();

        state.on_locate();
        assert_eq!(record(&mut state, true), Some(Notification::New(id)));
    }

    #[test]
    fn test_locate_leaves_shown_page_synced() {
        let mut state = DeviceState::new();
        draw_and_process(&mut state);

        state.on_locate();
        assert_eq!(state.changed_page(), None);
    }

    #[test]
    fn test_locate_keeps_held_page_hidden() {
        let mut state = DeviceState::new();
        state.on_hold_level(1);

        state.on_locate();
        assert_eq!(state.changed_page(), None);
    }

    #[test]
    fn test_closing_current_page_moves_to_new_page() {
        let mut state = DeviceState::new();
        let id = state.id().clone();
        draw_and_process(&mut state);

        state.on_plot_closed(&id);
        assert_ne!(state.id(), &id);
        assert!(!state.page.shown);
    }

    #[test]
    fn test_closing_held_page_removes_it() {
        let mut state = DeviceState::new();
        let first = state.id().clone();
        draw_and_process(&mut state);
        state.on_hold_level(1);
        draw_and_process(&mut state);
        state.on_new_page();
        assert_eq!(held_pages(&state), slice::from_ref(&first));

        state.on_plot_closed(&first);
        assert_eq!(held_pages(&state), []);
    }

    #[test]
    fn test_pre_hold_changes_released_device() {
        let mut state = DeviceState::new();
        state.on_draw();
        assert_eq!(state.take_pre_hold_changes(), None);
    }

    #[test]
    fn test_pre_hold_changes_drawn_only_while_held() {
        let mut state = DeviceState::new();
        state.on_hold_level(1);
        state.on_draw();
        assert_eq!(state.take_pre_hold_changes(), None);
    }

    #[test]
    fn test_pre_hold_changes_deferred_page() {
        // The `devAskNewPage()` case: the page is drawn, held, and recorded by
        // `before.plot.new` before R asks for input
        let mut state = DeviceState::new();
        let id = state.id().clone();
        state.on_draw();
        state.on_hold_level(1);
        assert_eq!(record(&mut state, true), None);

        let Some(changes) = state.take_pre_hold_changes() else {
            panic!("Expected pre-hold changes");
        };
        assert_eq!(changes.id, id);
        assert!(!changes.needs_recording());
        assert_eq!(
            state.notify_pre_hold_changes(changes, true),
            PreHoldNotification {
                notification: Notification::New(id.clone()),
                redrawn: false,
            }
        );
        state.on_pre_hold_changes_sent(false);
        assert_eq!(state.changed_page(), None);

        // Pre-hold changes are only sent once per hold
        assert_eq!(state.take_pre_hold_changes(), None);
    }

    #[test]
    fn test_pre_hold_changes_drawn_page_needs_recording() {
        let mut state = DeviceState::new();
        let id = state.id().clone();
        state.on_draw();
        state.on_hold_level(1);

        let Some(changes) = state.take_pre_hold_changes() else {
            panic!("Expected pre-hold changes");
        };
        assert_eq!(changes.id, id);
        assert!(changes.needs_recording());
        assert_eq!(
            state.notify_pre_hold_changes(changes, true),
            PreHoldNotification {
                notification: Notification::New(id),
                redrawn: false,
            }
        );
    }

    #[test]
    fn test_pre_hold_changes_redrawn_page_is_sent_again_at_release() {
        let mut state = DeviceState::new();
        let id = state.id().clone();
        state.on_draw();
        state.on_hold_level(1);
        record(&mut state, true);
        state.on_draw();

        let Some(changes) = state.take_pre_hold_changes() else {
            panic!("Expected pre-hold changes");
        };
        assert_eq!(
            state.notify_pre_hold_changes(changes, true),
            PreHoldNotification {
                notification: Notification::New(id.clone()),
                redrawn: true,
            }
        );
        state.on_pre_hold_changes_sent(true);

        state.on_hold_level(0);
        assert_eq!(record(&mut state, true), Some(Notification::Update(id)));
    }

    #[test]
    fn test_pre_hold_changes_keep_deferral_from_render() {
        let mut state = DeviceState::new();
        let id = state.id().clone();
        state.on_draw();
        state.on_hold_level(1);
        record(&mut state, true);

        let Some(changes) = state.take_pre_hold_changes() else {
            panic!("Expected pre-hold changes");
        };
        let sent = state.notify_pre_hold_changes(changes, true);
        assert!(!sent.redrawn);

        // Rendering the notification draws back into our device, and the
        // nested `process_changes()` defers the page
        assert_eq!(draw_and_process(&mut state), None);
        state.on_pre_hold_changes_sent(sent.redrawn);
        assert_eq!(state.page.state, PageState::Deferred { redrawn: false });

        // The page is shown, so a new page while held still owes it an update
        state.on_new_page();
        assert_eq!(held_pages(&state), slice::from_ref(&id));
    }

    #[test]
    fn test_pre_hold_changes_survive_plot_closed_during_render() {
        let mut state = DeviceState::new();
        let id = state.id().clone();
        state.on_draw();
        state.on_hold_level(1);
        record(&mut state, true);
        state.on_draw();

        let Some(changes) = state.take_pre_hold_changes() else {
            panic!("Expected pre-hold changes");
        };
        let sent = state.notify_pre_hold_changes(changes, true);
        assert!(sent.redrawn);

        // The frontend closes the plot while it renders
        state.on_plot_closed(&id);
        state.on_pre_hold_changes_sent(sent.redrawn);

        // The drawing done while held goes out as a new plot at release
        let new_id = state.id().clone();
        assert_ne!(new_id, id);
        state.on_hold_level(0);
        assert_eq!(record(&mut state, true), Some(Notification::New(new_id)));
    }

    #[test]
    fn test_new_page_clears_pre_hold_changes() {
        let mut state = DeviceState::new();
        draw_and_process(&mut state);
        state.on_draw();
        state.on_hold_level(1);
        state.on_new_page();

        assert_eq!(state.take_pre_hold_changes(), None);
    }
}
