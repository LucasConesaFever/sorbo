//! The workspace row in Apple's menu bar (BAR-1 to BAR-6, spec section 4).
//!
//! One `NSStatusItem` whose button holds a custom view, `SorboRowView`, made with
//! `define_class!` (no `block2`, so no new dependency). The view draws the row in
//! `drawRect:`, where AppKit has made the menu bar's own appearance current, so adaptive
//! colours resolve for a light or dark menu bar (BAR-6). Box positions and click hit testing
//! come from the pure `bar` module, so drawing and clicking can't disagree.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSFont, NSFontAttributeName, NSFontWeightSemibold,
    NSForegroundColorAttributeName, NSResponder, NSStatusBar, NSStatusItem, NSStringDrawing,
    NSVariableStatusItemLength, NSView,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSObject, NSPoint, NSRect, NSSize, NSString,
};
use wm_core::{Action, Event, Mode, WorkspaceState, WorkspaceStatus};

use super::{Shared, banner, run_or_defer};
use crate::bar::{
    BOX_HEIGHT, BOX_WIDTH, CORNER_RADIUS, DOT_SIZE, MARGIN, box_x, dot_x, hit, hit_dot, row_offset,
    row_width_with_label,
};
use crate::wm_log;

/// Point size of the numbers, a little under the menu bar's 13 pt so they fit in the boxes.
const FONT_SIZE: f64 = 12.0;
/// Shown after the row while the resize mode is on (MODE-4).
const RESIZE_LABEL: &str = "resize";

/// The row's font: semibold, with digits of equal width so the boxes don't jitter.
fn row_font() -> Retained<NSFont> {
    // SAFETY: reading an extern static provided by AppKit.
    NSFont::monospacedDigitSystemFontOfSize_weight(FONT_SIZE, unsafe { NSFontWeightSemibold })
}

pub(crate) struct Ivars {
    /// The shared-state pointer (see `mod.rs`), for pushing click events.
    shared: *const c_void,
    /// The row being shown.
    row: RefCell<Vec<WorkspaceStatus>>,
    /// Show the red "config has errors" dot after the row (CFG-6).
    dot: Cell<bool>,
    /// The key mode; `Resize` shows the label after the row and dot (MODE-4).
    mode: Cell<Mode>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements we break: we only override drawing and
    // event methods with their documented signatures, and this class doesn't implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SorboRowView"]
    #[ivars = Ivars]
    pub(crate) struct RowView;

    impl RowView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        /// The menu bar turned light or dark (it follows the wallpaper): redraw (BAR-6).
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            self.setNeedsDisplay(true);
        }

        /// sorbo is never the active app, so the first click must count.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        /// Not calling `super` keeps the button from highlighting the whole row.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            // The boxes and dot start after the mode label, when it's shown.
            let x = point.x - row_offset(self.label_width());
            let count = self.ivars().row.borrow().len();
            let on_dot = self.ivars().dot.get() && hit_dot(count, x);
            let workspace = hit(&self.ivars().row.borrow(), x);
            if !on_dot && workspace.is_none() {
                return;
            }
            let work = move |s: &mut Shared| {
                if on_dot {
                    // The dot brings back a dismissed error banner.
                    s.banner_state.show_again();
                    banner::refresh(s);
                } else if let Some(n) = workspace {
                    s.push(Event::Action(Action::GoToWorkspace(n)));
                }
            };
            // SAFETY: the pointer was made with `info_ptr` (see `MacPlatform::new`).
            unsafe { run_or_defer(self.ivars().shared, "menu bar click", work) };
        }
    }
);

impl RowView {
    /// The mode label's width while the resize mode is on (MODE-4).
    fn label_width(&self) -> Option<f64> {
        (self.ivars().mode.get() == Mode::Resize)
            .then(|| text_size(RESIZE_LABEL, &row_font()).width)
    }

    fn draw(&self) {
        let row = self.ivars().row.borrow();
        let y = (self.bounds().size.height - BOX_HEIGHT) / 2.0;
        let font = row_font();
        let label_width = self.label_width();
        let offset = row_offset(label_width);
        for (i, ws) in row.iter().enumerate() {
            let rect = NSRect::new(
                NSPoint::new(offset + box_x(i), y),
                NSSize::new(BOX_WIDTH, BOX_HEIGHT),
            );
            // Colours are looked up here, while AppKit has the menu bar's appearance current.
            let (fill, text) = match ws.state {
                WorkspaceState::Focused => {
                    (Some(NSColor::controlAccentColor()), NSColor::whiteColor())
                }
                WorkspaceState::Visible => {
                    (Some(NSColor::quaternaryLabelColor()), NSColor::labelColor())
                }
                WorkspaceState::Hidden => (None, NSColor::secondaryLabelColor()),
            };
            if let Some(fill) = fill {
                fill.setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect,
                    CORNER_RADIUS,
                    CORNER_RADIUS,
                )
                .fill();
            }
            draw_centered(&ws.number.to_string(), &font, &text, rect);
        }
        if self.ivars().dot.get() {
            let dot = NSRect::new(
                NSPoint::new(
                    offset + dot_x(row.len()),
                    (self.bounds().size.height - DOT_SIZE) / 2.0,
                ),
                NSSize::new(DOT_SIZE, DOT_SIZE),
            );
            NSColor::systemRedColor().setFill();
            NSBezierPath::bezierPathWithOvalInRect(dot).fill();
        }
        if let Some(width) = label_width {
            let rect = NSRect::new(NSPoint::new(MARGIN, y), NSSize::new(width, BOX_HEIGHT));
            draw_centered(RESIZE_LABEL, &font, &NSColor::labelColor(), rect);
        }
    }
}

fn attributes(
    font: &NSFont,
    color: &NSColor,
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    // SAFETY: reading extern statics provided by AppKit.
    let keys: [&NSAttributedStringKey; 2] =
        unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let values: [&AnyObject; 2] = [font, color];
    NSDictionary::from_slices(&keys, &values)
}

/// How big `text` is when drawn in `font`.
fn text_size(text: &str, font: &NSFont) -> NSSize {
    let attrs = attributes(font, &NSColor::labelColor());
    // SAFETY: the attributes are a font and a colour under their documented keys.
    unsafe { NSString::from_str(text).sizeWithAttributes(Some(&attrs)) }
}

/// Draws `text` centred in `rect`.
fn draw_centered(text: &str, font: &NSFont, color: &NSColor, rect: NSRect) {
    let attrs = attributes(font, color);
    let string = NSString::from_str(text);
    // SAFETY: the attributes are a font and a colour under their documented keys.
    unsafe {
        let size = string.sizeWithAttributes(Some(&attrs));
        let origin = NSPoint::new(
            rect.origin.x + (rect.size.width - size.width) / 2.0,
            rect.origin.y + (rect.size.height - size.height) / 2.0,
        );
        string.drawAtPoint_withAttributes(origin, Some(&attrs));
    }
}

/// The status item and its view. Owned by `MacPlatform`, so the item stays in the menu bar
/// for the life of the process and disappears with it.
pub(crate) struct StatusBar {
    item: Retained<NSStatusItem>,
    view: Retained<RowView>,
}

impl StatusBar {
    /// Adds an empty item to the menu bar. `shared` is `Rc::as_ptr` of the shared state.
    pub(crate) fn new(mtm: MainThreadMarker, shared: *const c_void) -> StatusBar {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let view = RowView::alloc(mtm).set_ivars(Ivars {
            shared,
            row: RefCell::new(Vec::new()),
            dot: Cell::new(false),
            mode: Cell::new(Mode::Normal),
        });
        // SAFETY: calling NSView's designated initializer on a freshly allocated object.
        let view: Retained<RowView> =
            unsafe { msg_send![super(view), initWithFrame: NSRect::ZERO] };
        match item.button(mtm) {
            Some(button) => button.addSubview(&view),
            None => wm_log!("menu bar: status item has no button; the row won't show"),
        }
        item.setLength(0.0);
        StatusBar { item, view }
    }

    /// Shows `row`, resizing the item to fit it (an empty row takes no space).
    pub(crate) fn show(&self, row: Vec<WorkspaceStatus>) {
        *self.view.ivars().row.borrow_mut() = row;
        self.resize();
    }

    /// Shows or hides the red dot after the row.
    pub(crate) fn set_dot(&self, dot: bool) {
        if self.view.ivars().dot.replace(dot) != dot {
            self.resize();
        }
    }

    /// Shows the resize label after the row while `mode` is `Resize` (MODE-4).
    pub(crate) fn set_mode(&self, mode: Mode) {
        if self.view.ivars().mode.replace(mode) != mode {
            self.resize();
        }
    }

    /// Fits the item to the row, the dot and the mode label, and redraws.
    fn resize(&self) {
        let ivars = self.view.ivars();
        let count = ivars.row.borrow().len();
        let width = row_width_with_label(count, ivars.dot.get(), self.view.label_width());
        self.item.setLength(width);
        let height = NSStatusBar::systemStatusBar().thickness();
        self.view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, height),
        ));
        self.view.setNeedsDisplay(true);
    }
}
