//! Check for Updates…: a small, non-modal window that follows a manual check from
//! progress to its outcome. Automatic checks never open it.

use std::cell::RefCell;

use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained, runtime::AnyObject, sel};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSColor, NSFont, NSProgressIndicator,
    NSProgressIndicatorStyle, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::update::{self, Outcome, Step};

const WIDTH: f64 = 400.0;
const HEIGHT: f64 = 170.0;

#[derive(Clone, Debug, PartialEq)]
enum Shown {
    Working(Step),
    Ready(String),
    UpToDate,
    Failed(String),
}

struct UpdateWindow {
    window: Retained<NSWindow>,
    headline: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    bar: Retained<NSProgressIndicator>,
    dismiss: Retained<NSButton>,
    restart: Retained<NSButton>,
    shown: Option<Shown>,
}

thread_local! {
    static WINDOW: RefCell<Option<UpdateWindow>> = const { RefCell::new(None) };
}

fn frame(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

impl UpdateWindow {
    /// `target` is the process-lifetime ticker, which implements both button actions.
    unsafe fn new(mtm: MainThreadMarker, target: *const AnyObject) -> Self {
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        // SAFETY: these are the designated initializer's documented arguments.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame(0.0, 0.0, WIDTH, HEIGHT),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("Software Update"));
        // The thread-local owner reuses this window after its close button hides it.
        unsafe { window.setReleasedWhenClosed(false) };

        let headline = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        headline.setFrame(frame(20.0, 120.0, WIDTH - 40.0, 24.0));
        headline.setFont(Some(&NSFont::boldSystemFontOfSize(15.0)));

        let detail = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        detail.setFrame(frame(20.0, 70.0, WIDTH - 40.0, 44.0));
        detail.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        detail.setTextColor(Some(&NSColor::secondaryLabelColor()));

        let bar = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            frame(20.0, 53.0, WIDTH - 40.0, 6.0),
        );
        bar.setStyle(NSProgressIndicatorStyle::Bar);
        bar.setMinValue(0.0);
        bar.setMaxValue(100.0);

        // SAFETY: target outlives the window and both selectors are implemented by it.
        let (dismiss, restart) = unsafe {
            let target = target.as_ref();
            (
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("Hide"),
                    target,
                    Some(sel!(dismissUpdate:)),
                    mtm,
                ),
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("Restart Now"),
                    target,
                    Some(sel!(restartToUpdate:)),
                    mtm,
                ),
            )
        };
        dismiss.setFrame(frame(WIDTH - 100.0, 16.0, 80.0, 26.0));
        dismiss.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        restart.setFrame(frame(WIDTH - 224.0, 16.0, 116.0, 26.0));
        restart.setHidden(true);

        let content = window
            .contentView()
            .expect("an NSWindow has a content view");
        content.addSubview(&headline);
        content.addSubview(&detail);
        content.addSubview(&bar);
        content.addSubview(&dismiss);
        content.addSubview(&restart);
        Self {
            window,
            headline,
            detail,
            bar,
            dismiss,
            restart,
            shown: None,
        }
    }

    fn render(&mut self, shown: Shown) {
        if self.shown.as_ref() == Some(&shown) {
            return;
        }
        let working = matches!(shown, Shown::Working(_));
        let ready = matches!(shown, Shown::Ready(_));
        let (headline, detail) = words(&shown);
        self.headline.setStringValue(&NSString::from_str(&headline));
        self.detail.setStringValue(&NSString::from_str(&detail));

        self.bar.setHidden(!working);
        let fraction = match &shown {
            Shown::Working(Step::Downloading {
                got,
                total: Some(total),
                ..
            }) if *total > 0 => Some(download_fraction(*got, *total)),
            _ => None,
        };
        self.bar.setIndeterminate(working && fraction.is_none());
        if let Some(fraction) = fraction {
            // SAFETY: a determinate bar doesn't animate; nil is a valid sender anyway.
            unsafe { self.bar.stopAnimation(None) };
            self.bar.setDoubleValue(fraction * 100.0);
        } else if working {
            // SAFETY: nil is a valid sender for progress animation.
            unsafe { self.bar.startAnimation(None) };
        } else {
            // SAFETY: nil is a valid sender for progress animation.
            unsafe { self.bar.stopAnimation(None) };
        }

        self.restart.setHidden(!ready);
        self.restart
            .setKeyEquivalent(&NSString::from_str(if ready { "\r" } else { "" }));
        self.dismiss.setTitle(&NSString::from_str(match shown {
            Shown::Working(_) => "Hide",
            Shown::Ready(_) => "Later",
            Shown::UpToDate | Shown::Failed(_) => "OK",
        }));
        self.dismiss
            .setKeyEquivalent(&NSString::from_str(if ready { "" } else { "\r" }));
        self.dismiss.setFrame(frame(
            if ready { WIDTH - 224.0 } else { WIDTH - 100.0 },
            16.0,
            80.0,
            26.0,
        ));
        if ready {
            self.restart
                .setFrame(frame(WIDTH - 136.0, 16.0, 116.0, 26.0));
        }
        self.shown = Some(shown);
    }

    fn present(&self, mtm: MainThreadMarker) {
        NSApplication::sharedApplication(mtm).activate();
        self.window.makeKeyAndOrderFront(None);
    }
}

/// Opens the one update window, or brings it back while its check continues.
///
/// # Safety
/// `target` must be the process-lifetime ticker and implement the button selectors.
pub unsafe fn show(mtm: MainThreadMarker, target: *const AnyObject) {
    WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            // SAFETY: the caller guarantees the ticker's lifetime and selectors.
            let mut created = unsafe { UpdateWindow::new(mtm, target) };
            created.render(Shown::Working(
                update::current_step().unwrap_or(Step::Checking),
            ));
            created.window.center();
            *slot = Some(created);
        }
        let window = slot.as_mut().expect("the window was just created");
        window.render(Shown::Working(
            update::current_step().unwrap_or(Step::Checking),
        ));
        window.present(mtm);
    });
}

/// Follows an open manual check; a completed check reopens a window hidden with Hide.
pub fn refresh(mtm: MainThreadMarker) {
    let outcome = update::take_notified_outcome();
    WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return;
        };
        if let Some(outcome) = outcome {
            window.render(match outcome {
                Outcome::Ready(version) => Shown::Ready(version),
                Outcome::UpToDate => Shown::UpToDate,
                Outcome::Failed(err) => Shown::Failed(err),
            });
            window.present(mtm);
        } else if window.window.isVisible()
            && matches!(window.shown.as_ref(), Some(Shown::Working(_)))
            && let Some(step) = update::current_step()
        {
            window.render(Shown::Working(step));
        }
    });
}

pub fn dismiss() {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow().as_ref() {
            window.window.orderOut(None);
        }
    });
}

fn words(shown: &Shown) -> (String, String) {
    match shown {
        Shown::Working(Step::Checking) => (
            String::from("Checking for updates…"),
            concat!("You have stats-lite ", env!("CARGO_PKG_VERSION"), ".").to_owned(),
        ),
        Shown::Working(Step::Downloading {
            version,
            got,
            total,
        }) => (
            format!("Downloading stats-lite {version}…"),
            match total {
                Some(total) => format!("{} of {}", megabytes(*got), megabytes(*total)),
                None => megabytes(*got),
            },
        ),
        Shown::Working(Step::Verifying { version }) => (
            format!("Verifying stats-lite {version}…"),
            String::from("Checking its signature and Apple's notarization."),
        ),
        Shown::Ready(version) => (
            format!("stats-lite {version} is ready"),
            String::from("Restart now, or it installs when stats-lite quits."),
        ),
        Shown::UpToDate => (
            String::from("stats-lite is up to date"),
            concat!(env!("CARGO_PKG_VERSION"), " is the newest version.").to_owned(),
        ),
        Shown::Failed(err) => (
            String::from("stats-lite can't update right now"),
            err.clone(),
        ),
    }
}

fn download_fraction(got: u64, total: u64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a progress ratio needs no more than f64's 53 bits"
    )]
    {
        (got as f64 / total as f64).clamp(0.0, 1.0)
    }
}

/// Decimal, as Finder counts.
fn megabytes(bytes: u64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "shown to one decimal; a download is far below 2^53 bytes"
    )]
    {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_sizes_should_read_as_finder_shows_them() {
        assert_eq!(
            (megabytes(0), megabytes(4_570_610), download_fraction(5, 4)),
            ("0.0 MB".to_owned(), "4.6 MB".to_owned(), 1.0)
        );
    }
}
