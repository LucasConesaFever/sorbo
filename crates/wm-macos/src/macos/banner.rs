//! The red config error banner (CFG-6, CFG-9, spec section 6).
//!
//! A borderless, non-activating `NSPanel` across the top of the main display, under the
//! menu bar. It's shown with `orderFrontRegardless` and never made key, so it never
//! activates sorbo or takes focus from the frontmost app (popups stay open). A click
//! dismisses it. What it says comes from the pure `BannerState`.

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSPanel, NSResponder, NSScreen, NSStatusWindowLevel,
    NSStringDrawing, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSObject, NSPoint, NSRect, NSSize, NSString,
};

use super::{Shared, run_or_defer};
use crate::banner_state::wrap_line;

const FONT_SIZE: f64 = 13.0;
const LINE_HEIGHT: f64 = 18.0;
const PADDING: f64 = 6.0;
/// Space before the text.
const INSET: f64 = 12.0;

pub(crate) struct Ivars {
    /// The shared-state pointer (see `mod.rs`), for handling clicks.
    shared: *const c_void,
    /// The lines to draw, already wrapped to the banner's width, and whether each is bold.
    lines: RefCell<Vec<(String, bool)>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements we break: we only override drawing and
    // event methods with their documented signatures, and this class doesn't implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SorboBannerView"]
    #[ivars = Ivars]
    pub(crate) struct BannerView;

    impl BannerView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        /// sorbo is never the active app, so the first click must count.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        /// A click dismisses the banner; the menu bar dot brings it back.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let work = |s: &mut Shared| {
                s.banner_state.dismiss();
                refresh(s);
            };
            // SAFETY: the pointer was made with `info_ptr` (see `MacPlatform::new`).
            unsafe { run_or_defer(self.ivars().shared, "banner click", work) };
        }
    }
);

impl BannerView {
    fn draw(&self) {
        let bounds = self.bounds();
        NSColor::systemRedColor().setFill();
        objc2_app_kit::NSRectFill(bounds);
        let lines = self.ivars().lines.borrow();
        let white = NSColor::whiteColor();
        for (i, (line, bold)) in lines.iter().enumerate() {
            let y = bounds.size.height - PADDING - (i + 1) as f64 * LINE_HEIGHT;
            draw_text(line, &font(*bold), &white, NSPoint::new(INSET, y));
        }
    }
}

/// The summary (first) line is bold; the errors follow in the regular weight.
fn font(bold: bool) -> Retained<NSFont> {
    if bold {
        NSFont::boldSystemFontOfSize(FONT_SIZE)
    } else {
        NSFont::systemFontOfSize(FONT_SIZE)
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

fn draw_text(text: &str, font: &NSFont, color: &NSColor, origin: NSPoint) {
    let attrs = attributes(font, color);
    // SAFETY: the attributes are a font and a colour under their documented keys.
    unsafe { NSString::from_str(text).drawAtPoint_withAttributes(origin, Some(&attrs)) };
}

/// How wide `text` is when drawn in `font`.
fn text_width(text: &str, font: &NSFont) -> f64 {
    let attrs = attributes(font, &NSColor::whiteColor());
    // SAFETY: the attributes are a font and a colour under their documented keys.
    unsafe { NSString::from_str(text).sizeWithAttributes(Some(&attrs)) }.width
}

/// `lines` wrapped to `max_width` (CFG-6); every piece of the first line stays bold.
fn wrapped(lines: Vec<String>, max_width: f64) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let bold = i == 0;
        let font = font(bold);
        for piece in wrap_line(line, max_width, |t| text_width(t, &font)) {
            out.push((piece, bold));
        }
    }
    out
}

/// The panel and its view. Lives in `Shared` for the life of the process.
pub(crate) struct Banner {
    mtm: MainThreadMarker,
    panel: Retained<NSPanel>,
    view: Retained<BannerView>,
}

impl Banner {
    /// Creates the (hidden) panel. `shared` is `info_ptr` of the shared state.
    pub(crate) fn new(mtm: MainThreadMarker, shared: *const c_void) -> Banner {
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            NSRect::ZERO,
            style,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: the panel is owned by `Banner` (a `Retained`), not released on close.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        // Above normal and floating windows, like the menu bar's own items.
        panel.setLevel(NSStatusWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setHasShadow(false);
        let view = BannerView::alloc(mtm).set_ivars(Ivars {
            shared,
            lines: RefCell::new(Vec::new()),
        });
        // SAFETY: calling NSView's designated initializer on a freshly allocated object.
        let view: Retained<BannerView> =
            unsafe { msg_send![super(view), initWithFrame: NSRect::ZERO] };
        panel.setContentView(Some(&view));
        Banner { mtm, panel, view }
    }

    /// Shows `lines` across the top of the main display, just under the menu bar, wrapped
    /// to its width. Called again after display changes, so it follows the main display.
    fn show(&self, lines: Vec<String>) {
        // The first screen is the one with the menu bar.
        let Some(screen) = NSScreen::screens(self.mtm).iter().next() else {
            return;
        };
        let frame = screen.frame();
        let visible = screen.visibleFrame();
        let lines = wrapped(lines, frame.size.width - 2.0 * INSET);
        let height = 2.0 * PADDING + lines.len() as f64 * LINE_HEIGHT;
        let top = visible.origin.y + visible.size.height;
        let rect = NSRect::new(
            NSPoint::new(frame.origin.x, top - height),
            NSSize::new(frame.size.width, height),
        );
        *self.view.ivars().lines.borrow_mut() = lines;
        self.panel.setFrame_display(rect, true);
        self.view.setNeedsDisplay(true);
        self.panel.orderFrontRegardless();
    }

    fn hide(&self) {
        self.panel.orderOut(None);
    }
}

/// Makes the banner and the menu bar dot match `s.banner_state` now.
pub(crate) fn refresh(s: &mut Shared) {
    let lines = s.banner_state.lines(Instant::now());
    if let Some(banner) = &s.banner {
        if lines.is_empty() {
            banner.hide();
        } else {
            banner.show(lines);
        }
    }
    if let Some(bar) = &s.status_bar {
        bar.set_dot(s.banner_state.dot());
    }
}
